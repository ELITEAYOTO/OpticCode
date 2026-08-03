use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use opticcode_tools::eval::{
    evaluation_fixture_fingerprint, load_eval_suite, summarize_results, EvalCase, EvalCaseStatus,
    EvalExpected, EvalFixture, EvalHumanReview, EvalLlmGenerationMetrics, EvalLlmGenerationStatus,
    EvalRunReport, EvalStrategy,
};

use crate::{AskOptions, ContextFallbackPolicy, ContextMode, OpticCode, DEFAULT_PROFILE};

#[derive(Debug, Clone, Default)]
pub struct EvalLlmRuntimeOptions {
    pub external_fixtures: BTreeMap<String, PathBuf>,
    pub rag_index: Option<PathBuf>,
}

pub async fn enrich_evaluation_with_llm(
    report: &mut EvalRunReport,
    suite_path: &Path,
    options: EvalLlmRuntimeOptions,
) -> Result<()> {
    let generation = report
        .configuration
        .generation
        .clone()
        .context("evaluation report has no LLM generation configuration")?;
    if !generation.provider.eq_ignore_ascii_case("ollama") {
        bail!(
            "CONTEXT-002 evaluation supports only the local Ollama provider, got `{}`",
            generation.provider
        );
    }
    if generation.endpoint.trim().is_empty() {
        bail!("evaluation Ollama endpoint must not be empty");
    }
    let suite_path = fs::canonicalize(suite_path).with_context(|| {
        format!(
            "failed to resolve evaluation suite path: {}",
            suite_path.display()
        )
    })?;
    let suite = load_eval_suite(&suite_path)?;
    let suite_dir = suite_path
        .parent()
        .context("evaluation suite path has no parent directory")?;
    let fixtures = resolve_fixtures(&suite.fixtures, suite_dir, &options.external_fixtures)?;
    let fingerprints = fingerprint_available_fixtures(&fixtures)?;
    let cases = suite
        .cases
        .into_iter()
        .map(|case| (case.id.clone(), case))
        .collect::<BTreeMap<_, _>>();

    let app = OpticCode::try_new(&generation.endpoint, generation.model.clone())?
        .with_keep_alive(generation.keep_alive.clone())
        .with_http_timeout(Duration::from_millis(generation.http_timeout_ms))?;
    if let Err(error) = app.ensure_model_available().await {
        mark_llm_unavailable(report, &format!("{error:#}"));
        verify_fingerprints(&fixtures, &fingerprints)?;
        report.summary = summarize_results(report.summary.case_count, &report.results);
        return Ok(());
    }

    for warmup in 0..generation.warmup_runs {
        let Some((case, fixture, strategy)) = first_generation_target(report, &cases, &fixtures)
        else {
            break;
        };
        let result =
            run_eval_generation(&app, case, fixture, strategy, &generation, &options, false)
                .await?;
        if result.metrics.status != EvalLlmGenerationStatus::Generated {
            report.warnings.push(format!(
                "LLM warmup {} of {} did not complete: {}",
                warmup + 1,
                generation.warmup_runs,
                result
                    .metrics
                    .error
                    .as_deref()
                    .or(result.metrics.skip_reason.as_deref())
                    .unwrap_or("unknown reason")
            ));
            break;
        }
    }

    let llm_started = Instant::now();
    let mut attempted = 0usize;
    for result in &mut report.results {
        if result.status != EvalCaseStatus::Completed
            || !matches!(result.strategy, EvalStrategy::Legacy | EvalStrategy::Symbol)
        {
            continue;
        }
        let Some(case) = cases.get(&result.case_id) else {
            result.metrics.generation = Some(skipped_metrics(
                result.strategy,
                false,
                "evaluation case metadata is unavailable",
            ));
            continue;
        };
        let Some(Some(fixture)) = fixtures.get(&result.fixture) else {
            result.metrics.generation = Some(skipped_metrics(
                result.strategy,
                false,
                "optional evaluation fixture is unavailable",
            ));
            continue;
        };
        let cold_candidate = generation.warmup_runs == 0 && attempted == 0;
        attempted = attempted.saturating_add(1);
        let generated = run_eval_generation(
            &app,
            case,
            fixture,
            result.strategy,
            &generation,
            &options,
            cold_candidate,
        )
        .await?;
        result.metrics.context.actual_prompt_tokens = generated.metrics.actual_prompt_tokens;
        result.metrics.context.generated_tokens = generated.metrics.generated_tokens;
        result.metrics.generation = Some(generated.metrics);
        if let Some(response) = generated.response {
            result.metrics.response = evaluate_response(&case.expected, &response);
            result.response = Some(response);
        }
    }
    report.duration_us = report
        .duration_us
        .saturating_add(duration_us(llm_started.elapsed()));
    report
        .warnings
        .retain(|warning| !warning.contains("generation must be supplied by CONTEXT-002"));
    report.warnings.push(format!(
        "CONTEXT-002 LLM enrichment attempted {attempted} legacy/symbol generations; cold_candidate is observational and does not guarantee an unloaded model"
    ));
    verify_fingerprints(&fixtures, &fingerprints)?;
    report.summary = summarize_results(report.summary.case_count, &report.results);
    Ok(())
}

struct GeneratedEvalCase {
    metrics: EvalLlmGenerationMetrics,
    response: Option<String>,
}

async fn run_eval_generation(
    app: &OpticCode,
    case: &EvalCase,
    fixture: &Path,
    strategy: EvalStrategy,
    generation: &opticcode_tools::eval::EvalGenerationConfiguration,
    runtime: &EvalLlmRuntimeOptions,
    cold_candidate: bool,
) -> Result<GeneratedEvalCase> {
    let mode = match strategy {
        EvalStrategy::Legacy => ContextMode::Legacy,
        EvalStrategy::Symbol => ContextMode::Symbol,
        _ => bail!("LLM generation requires legacy or symbol context"),
    };
    let include_rag = runtime.rag_index.is_some();
    let rag_index = runtime
        .rag_index
        .clone()
        .unwrap_or_else(|| PathBuf::from("data/index"));
    let assistant = app
        .ask_with_report(AskOptions {
            workspace: fixture.to_path_buf(),
            prompt: case.prompt.clone(),
            profile: Some(DEFAULT_PROFILE.to_string()),
            include_memory: false,
            include_rag,
            rag_index,
            rag_limit: 4,
            brief: false,
            max_tokens: Some(generation.max_generated_tokens),
            temperature: generation.temperature,
            seed: generation.seed,
            context_mode: mode,
            fallback_policy: ContextFallbackPolicy::Refuse,
            compare_generate: false,
            verify_model: false,
        })
        .await?;
    let Some(run) = assistant.runs.iter().find(|run| run.context_mode == mode) else {
        return Ok(GeneratedEvalCase {
            metrics: failed_metrics(
                strategy,
                cold_candidate,
                assistant.preparation_duration_us,
                "assistant report contains no matching context run",
            ),
            response: None,
        });
    };
    if !assistant.success || !run.generated {
        let reason = run
            .error
            .as_ref()
            .or_else(|| assistant.errors.first())
            .map(|error| format!("{}: {}", error.code, error.message))
            .or_else(|| run.skipped_reason.clone())
            .unwrap_or_else(|| "generation did not complete".to_string());
        let context_rejected = run
            .error
            .as_ref()
            .or_else(|| assistant.errors.first())
            .is_some_and(|error| error.code == "context_rejected");
        return Ok(GeneratedEvalCase {
            metrics: if context_rejected || run.skipped_reason.is_some() {
                skipped_metrics_with_context(
                    strategy,
                    cold_candidate,
                    assistant.preparation_duration_us,
                    run.prompt.estimated_tokens,
                    &reason,
                )
            } else {
                failed_metrics(
                    strategy,
                    cold_candidate,
                    assistant.preparation_duration_us,
                    &reason,
                )
            },
            response: None,
        });
    }
    let metrics = run
        .metrics
        .as_ref()
        .context("generated assistant run contains no metrics")?;
    Ok(GeneratedEvalCase {
        metrics: EvalLlmGenerationMetrics {
            status: EvalLlmGenerationStatus::Generated,
            cold_candidate,
            context_mode: strategy,
            estimated_prompt_tokens: run.prompt.estimated_tokens,
            actual_prompt_tokens: metrics.prompt_eval_count,
            generated_tokens: metrics.generated_tokens,
            context_build_us: assistant.preparation_duration_us,
            client_total_us: Some(metrics.client_ms.saturating_mul(1_000)),
            provider_total_us: metrics
                .ollama_total_ms
                .map(|value| value.saturating_mul(1_000)),
            load_us: metrics
                .ollama_load_ms
                .map(|value| value.saturating_mul(1_000)),
            prompt_eval_us: metrics
                .prompt_eval_ms
                .map(|value| value.saturating_mul(1_000)),
            generation_us: metrics
                .generation_ms
                .map(|value| value.saturating_mul(1_000)),
            generated_tokens_per_second: metrics.generated_tokens_per_second,
            skip_reason: None,
            error: None,
        },
        response: run.response.clone(),
    })
}

fn evaluate_response(
    expected: &EvalExpected,
    response: &str,
) -> opticcode_tools::eval::EvalResponseMetrics {
    let normalized = response.to_ascii_lowercase().replace('\\', "/");
    let expected_facts_found = expected
        .facts
        .iter()
        .filter(|fact| deterministic_fact_match(&normalized, fact))
        .count();
    let forbidden_claims_found = expected
        .forbidden_claims
        .iter()
        .filter(|claim| deterministic_forbidden_match(&normalized, claim))
        .count();
    let referenced_expected_files = expected
        .relevant_files
        .iter()
        .filter(|path| {
            let path = path.to_ascii_lowercase().replace('\\', "/");
            let name = path.rsplit('/').next().unwrap_or(&path);
            normalized.contains(&path) || normalized.contains(name)
        })
        .count();
    let referenced_expected_symbols = expected
        .relevant_symbols
        .iter()
        .filter(|symbol| {
            let symbol = symbol.to_ascii_lowercase();
            let member = symbol.rsplit(['#', '.']).next().unwrap_or(&symbol);
            normalized.contains(&symbol) || normalized.contains(member)
        })
        .count();
    let expected_total = expected
        .facts
        .len()
        .saturating_add(expected.relevant_files.len())
        .saturating_add(expected.relevant_symbols.len());
    let matched = expected_facts_found
        .saturating_add(referenced_expected_files)
        .saturating_add(referenced_expected_symbols);
    let base = if expected_total == 0 {
        1.0
    } else {
        matched as f64 / expected_total as f64
    };
    let quality = (base - forbidden_claims_found as f64 * 0.25).clamp(0.0, 1.0);
    opticcode_tools::eval::EvalResponseMetrics {
        generated: true,
        expected_facts_found,
        expected_facts_total: expected.facts.len(),
        forbidden_claims_found,
        referenced_expected_files,
        referenced_expected_symbols,
        deterministic_quality_score: Some(quality),
        build_validated: false,
        tests_validated: false,
        ast_validated: false,
        scope_preserved: true,
        human_review: EvalHumanReview::PendingHumanReview,
        experimental_llm_judge_score: None,
    }
}

fn deterministic_fact_match(normalized_response: &str, expected: &str) -> bool {
    let expected_normalized = expected.to_ascii_lowercase().replace('\\', "/");
    if normalized_response.contains(&expected_normalized) {
        return true;
    }
    let expected_keywords = significant_keywords(&expected_normalized);
    if expected_keywords.is_empty() {
        return false;
    }
    let response_keywords = significant_keywords(normalized_response);
    let matched = expected_keywords.intersection(&response_keywords).count();
    matched.saturating_mul(4) >= expected_keywords.len().saturating_mul(3)
}

fn deterministic_forbidden_match(normalized_response: &str, forbidden: &str) -> bool {
    let forbidden = forbidden.to_ascii_lowercase().replace('\\', "/");
    if normalized_response.contains(&forbidden) {
        return true;
    }
    let keywords = significant_keywords(&forbidden);
    let response_keywords = significant_keywords(normalized_response);
    !keywords.is_empty()
        && keywords
            .iter()
            .all(|keyword| response_keywords.contains(keyword))
}

fn significant_keywords(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter_map(|token| {
            let token = token.trim().to_ascii_lowercase();
            if token.is_empty()
                || matches!(
                    token.as_str(),
                    "a" | "an"
                        | "and"
                        | "as"
                        | "at"
                        | "by"
                        | "for"
                        | "from"
                        | "in"
                        | "is"
                        | "of"
                        | "on"
                        | "the"
                        | "through"
                        | "to"
                        | "with"
                )
                || (token.len() <= 2 && token != "op")
            {
                return None;
            }
            Some(
                token
                    .strip_suffix('s')
                    .filter(|stem| stem.len() >= 4)
                    .unwrap_or(&token)
                    .to_string(),
            )
        })
        .collect()
}

fn resolve_fixtures(
    fixtures: &BTreeMap<String, EvalFixture>,
    suite_dir: &Path,
    external: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, Option<PathBuf>>> {
    let mut resolved = BTreeMap::new();
    for (name, fixture) in fixtures {
        let root = match fixture {
            EvalFixture::Versioned { path } => {
                let path = fs::canonicalize(suite_dir.join(path))?;
                if !path.is_dir() {
                    bail!("versioned evaluation fixture `{name}` is not a directory");
                }
                Some(path)
            }
            EvalFixture::External { external_id, .. } => external
                .get(external_id)
                .and_then(|path| fs::canonicalize(path).ok())
                .filter(|path| path.is_dir()),
        };
        resolved.insert(name.clone(), root);
    }
    Ok(resolved)
}

fn fingerprint_available_fixtures(
    fixtures: &BTreeMap<String, Option<PathBuf>>,
) -> Result<BTreeMap<String, String>> {
    fixtures
        .iter()
        .filter_map(|(name, root)| root.as_ref().map(|root| (name, root)))
        .map(|(name, root)| Ok((name.clone(), evaluation_fixture_fingerprint(root)?)))
        .collect()
}

fn verify_fingerprints(
    fixtures: &BTreeMap<String, Option<PathBuf>>,
    expected: &BTreeMap<String, String>,
) -> Result<()> {
    for (name, fingerprint) in expected {
        let root = fixtures
            .get(name)
            .and_then(Option::as_deref)
            .context("evaluation fixture disappeared during LLM evaluation")?;
        if evaluation_fixture_fingerprint(root)? != *fingerprint {
            bail!(
                "evaluation fixture `{name}` changed during LLM evaluation; report publication refused"
            );
        }
    }
    Ok(())
}

fn first_generation_target<'a>(
    report: &EvalRunReport,
    cases: &'a BTreeMap<String, EvalCase>,
    fixtures: &'a BTreeMap<String, Option<PathBuf>>,
) -> Option<(&'a EvalCase, &'a Path, EvalStrategy)> {
    report.results.iter().find_map(|result| {
        if result.status != EvalCaseStatus::Completed
            || !matches!(result.strategy, EvalStrategy::Legacy | EvalStrategy::Symbol)
        {
            return None;
        }
        Some((
            cases.get(&result.case_id)?,
            fixtures.get(&result.fixture)?.as_deref()?,
            result.strategy,
        ))
    })
}

fn mark_llm_unavailable(report: &mut EvalRunReport, reason: &str) {
    let reason = format!("local Ollama generation skipped: {reason}");
    for result in &mut report.results {
        if result.status == EvalCaseStatus::Completed
            && matches!(result.strategy, EvalStrategy::Legacy | EvalStrategy::Symbol)
        {
            result.metrics.generation = Some(skipped_metrics(result.strategy, false, &reason));
        }
    }
    report
        .warnings
        .retain(|warning| !warning.contains("generation must be supplied by CONTEXT-002"));
    report.warnings.push(reason);
}

fn skipped_metrics(
    strategy: EvalStrategy,
    cold_candidate: bool,
    reason: &str,
) -> EvalLlmGenerationMetrics {
    skipped_metrics_with_context(strategy, cold_candidate, 0, 0, reason)
}

fn skipped_metrics_with_context(
    strategy: EvalStrategy,
    cold_candidate: bool,
    context_build_us: u64,
    estimated_prompt_tokens: usize,
    reason: &str,
) -> EvalLlmGenerationMetrics {
    EvalLlmGenerationMetrics {
        status: EvalLlmGenerationStatus::Skipped,
        cold_candidate,
        context_mode: strategy,
        estimated_prompt_tokens,
        actual_prompt_tokens: None,
        generated_tokens: None,
        context_build_us,
        client_total_us: None,
        provider_total_us: None,
        load_us: None,
        prompt_eval_us: None,
        generation_us: None,
        generated_tokens_per_second: None,
        skip_reason: Some(reason.to_string()),
        error: None,
    }
}

fn failed_metrics(
    strategy: EvalStrategy,
    cold_candidate: bool,
    context_build_us: u64,
    error: &str,
) -> EvalLlmGenerationMetrics {
    EvalLlmGenerationMetrics {
        status: EvalLlmGenerationStatus::Failed,
        cold_candidate,
        context_mode: strategy,
        estimated_prompt_tokens: 0,
        actual_prompt_tokens: None,
        generated_tokens: None,
        context_build_us,
        client_total_us: None,
        provider_total_us: None,
        load_us: None,
        prompt_eval_us: None,
        generation_us: None,
        generated_tokens_per_second: None,
        skip_reason: None,
        error: Some(error.to_string()),
    }
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::{deterministic_fact_match, deterministic_forbidden_match};

    #[test]
    fn deterministic_facts_tolerate_formatting_and_small_wording_changes() {
        assert!(deterministic_fact_match(
            "Permission: opticmini.use; Default Value: op",
            "opticmini.use defaults to op"
        ));
        assert!(deterministic_fact_match(
            "Material.SULPHUR is the correct Bukkit constant for version 1.8.8",
            "Material.SULPHUR is the Bukkit 1.8.8 name"
        ));
        assert!(!deterministic_fact_match(
            "Search every ping reference before editing.",
            "ping is called through a static wildcard import"
        ));
    }

    #[test]
    fn forbidden_claims_require_all_significant_terms() {
        assert!(deterministic_forbidden_match(
            "Rename the custom Material.GUNPOWDER declaration.",
            "rename custom Material.GUNPOWDER"
        ));
        assert!(!deterministic_forbidden_match(
            "Replace Bukkit Material.GUNPOWDER only.",
            "rename custom Material.GUNPOWDER"
        ));
    }
}
