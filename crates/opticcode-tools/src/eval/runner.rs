use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use walkdir::{DirEntry, WalkDir};

use crate::java_context::{build_java_task_context, JavaTaskContextOptions};
use crate::{
    build_project_context_with_limits, load_active_rag_manifest, search_rag_index_report,
    search_workspace,
};

use super::metrics::{calculate_retrieval_metrics, normalize_path, summarize_results};
use super::{
    compare_reports, EvalCase, EvalCaseMetrics, EvalCaseResult, EvalCaseStatus, EvalConfiguration,
    EvalContextConfiguration, EvalContextMetrics, EvalEnvironment, EvalExpected,
    EvalGenerationConfiguration, EvalHumanReview, EvalLlmMode, EvalObserved, EvalRagConfiguration,
    EvalRagIdentity, EvalResponseMetrics, EvalRetrievedItem, EvalRunReport, EvalStrategy,
    EvalSuite, EVAL_SCHEMA_VERSION, EVAL_SUITE_SCHEMA_VERSION,
};

const DEFAULT_LEGACY_MAX_FILES: usize = 8;
const DEFAULT_LEGACY_MAX_BYTES_PER_FILE: usize = 4 * 1024;
const DEFAULT_EXACT_LIMIT: usize = 12;
const MAX_EVAL_CASES: usize = 10_000;
const MAX_EVAL_REPETITIONS: u32 = 20;
const MAX_FINGERPRINT_FILES: usize = 100_000;
const MAX_FINGERPRINT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct EvalRunOptions {
    pub strategies: Vec<EvalStrategy>,
    pub repetitions: u32,
    pub case_ids: Vec<String>,
    pub case_limit: Option<usize>,
    pub rag_index: Option<PathBuf>,
    pub rag_index_label: String,
    pub rag_limit: usize,
    pub external_fixtures: BTreeMap<String, PathBuf>,
    pub llm_mode: EvalLlmMode,
    pub generation: Option<EvalGenerationConfiguration>,
    pub baseline: Option<EvalRunReport>,
}

impl Default for EvalRunOptions {
    fn default() -> Self {
        Self {
            strategies: vec![
                EvalStrategy::Legacy,
                EvalStrategy::Symbol,
                EvalStrategy::Exact,
            ],
            repetitions: 1,
            case_ids: Vec::new(),
            case_limit: None,
            rag_index: None,
            rag_index_label: "not_configured".to_string(),
            rag_limit: 8,
            external_fixtures: BTreeMap::new(),
            llm_mode: EvalLlmMode::Disabled,
            generation: None,
            baseline: None,
        }
    }
}

#[derive(Debug, Clone)]
enum ResolvedFixture {
    Available(PathBuf),
    MissingExternal(String),
}

pub fn load_eval_suite(path: &Path) -> Result<EvalSuite> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read evaluation suite: {}", path.display()))?;
    let suite: EvalSuite = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse evaluation suite: {}", path.display()))?;
    validate_eval_suite(&suite)?;
    Ok(suite)
}

pub fn validate_eval_suite(suite: &EvalSuite) -> Result<()> {
    if suite.schema_version != EVAL_SUITE_SCHEMA_VERSION {
        bail!(
            "unsupported evaluation suite schema {}; expected {}",
            suite.schema_version,
            EVAL_SUITE_SCHEMA_VERSION
        );
    }
    if suite.id.trim().is_empty()
        || suite.version.trim().is_empty()
        || suite.description.trim().is_empty()
    {
        bail!("evaluation suite id, version, and description must not be empty");
    }
    if suite.fixtures.is_empty() {
        bail!("evaluation suite must define at least one fixture");
    }
    for (name, fixture) in &suite.fixtures {
        match fixture {
            super::EvalFixture::Versioned { path } => {
                if path.trim().is_empty() || Path::new(path).is_absolute() {
                    bail!(
                        "versioned evaluation fixture `{name}` must use a non-empty relative path"
                    );
                }
            }
            super::EvalFixture::External {
                external_id,
                description,
            } => {
                if external_id.trim().is_empty() || description.trim().is_empty() {
                    bail!("external evaluation fixture `{name}` requires an id and description");
                }
            }
        }
    }
    if suite.cases.is_empty() || suite.cases.len() > MAX_EVAL_CASES {
        bail!("evaluation suite must contain between 1 and {MAX_EVAL_CASES} cases");
    }
    let mut ids = BTreeSet::new();
    for case in &suite.cases {
        if case.id.trim().is_empty()
            || case.prompt.trim().is_empty()
            || case.reason.trim().is_empty()
        {
            bail!("evaluation cases require a non-empty id, prompt, and reason");
        }
        if !ids.insert(case.id.as_str()) {
            bail!("duplicate evaluation case id `{}`", case.id);
        }
        if !suite.fixtures.contains_key(&case.fixture) {
            bail!(
                "evaluation case `{}` references unknown fixture `{}`",
                case.id,
                case.fixture
            );
        }
        if case.context_budget.max_files == 0
            || case.context_budget.max_snippets == 0
            || case.context_budget.max_chars == 0
            || case.context_budget.max_estimated_tokens == 0
        {
            bail!("evaluation case `{}` has a zero context budget", case.id);
        }
        reject_duplicates(
            &case.expected.relevant_files,
            &format!("case `{}` relevant_files", case.id),
        )?;
        reject_duplicates(
            &case.expected.relevant_symbols,
            &format!("case `{}` relevant_symbols", case.id),
        )?;
    }
    Ok(())
}

pub fn run_evaluation(suite_path: &Path, mut options: EvalRunOptions) -> Result<EvalRunReport> {
    validate_run_options(&options)?;
    options.strategies.sort_unstable();
    options.strategies.dedup();

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
    let requested_case_ids = options.case_ids.iter().cloned().collect::<BTreeSet<_>>();
    for case_id in &requested_case_ids {
        if !suite.cases.iter().any(|case| &case.id == case_id) {
            bail!("requested evaluation case `{case_id}` does not exist in the suite");
        }
    }
    let mut selected_cases = suite
        .cases
        .iter()
        .filter(|case| requested_case_ids.is_empty() || requested_case_ids.contains(&case.id))
        .collect::<Vec<_>>();
    if let Some(limit) = options.case_limit {
        selected_cases.truncate(limit.min(selected_cases.len()));
    }
    let case_count = selected_cases.len();
    let suite_truncated = case_count < suite.cases.len();
    let started_at_unix_ms = unix_time_ms()?;
    let started = Instant::now();

    let fixtures = resolve_fixtures(&suite, suite_dir, &options.external_fixtures)?;
    let fingerprints_before = capture_fixture_fingerprints(&fixtures)?;
    let (rag_identity, rag_error) = resolve_rag_identity(options.rag_index.as_deref());
    let configuration = EvalConfiguration {
        strategies: options.strategies.clone(),
        repetitions: options.repetitions,
        case_ids: options.case_ids.clone(),
        case_limit: options.case_limit,
        suite_truncated,
        llm_mode: options.llm_mode,
        context: EvalContextConfiguration {
            legacy_max_files: DEFAULT_LEGACY_MAX_FILES,
            legacy_max_bytes_per_file: DEFAULT_LEGACY_MAX_BYTES_PER_FILE,
            symbol_max_files: 12,
            symbol_max_snippets: 12,
            symbol_max_chars: 24 * 1024,
            symbol_max_estimated_tokens: 6 * 1024,
            exact_limit: DEFAULT_EXACT_LIMIT,
        },
        rag: EvalRagConfiguration {
            enabled: options.rag_index.is_some(),
            index_label: options.rag_index_label.clone(),
            limit: options.rag_limit,
        },
        generation: options.generation.clone(),
    };
    let configuration_hash = configuration_hash(&suite, &configuration)?;
    let run_id = format!(
        "{}-{}",
        started_at_unix_ms,
        &configuration_hash[..12.min(configuration_hash.len())]
    );
    let mut results = Vec::new();

    for case in selected_cases {
        let fixture = fixtures
            .get(&case.fixture)
            .context("resolved fixture disappeared during evaluation")?;
        for strategy in &options.strategies {
            for repetition in 1..=options.repetitions {
                let result = match fixture {
                    ResolvedFixture::MissingExternal(reason) => {
                        skipped_result(case, *strategy, repetition, reason)
                    }
                    ResolvedFixture::Available(root) => run_case_strategy(
                        case,
                        root,
                        *strategy,
                        repetition,
                        &configuration,
                        RagCaseInput {
                            index: options.rag_index.as_deref(),
                            error: rag_error.as_deref(),
                        },
                    ),
                };
                results.push(result);
            }
        }
    }

    verify_fixture_fingerprints(&fixtures, &fingerprints_before)?;
    let summary = summarize_results(case_count, &results);
    let mut warnings = Vec::new();
    if suite_truncated {
        warnings.push(format!(
            "suite truncated by case limit: executed {case_count} of {} cases",
            suite.cases.len()
        ));
    }
    if let Some(error) = rag_error {
        warnings.push(format!("RAG evaluation unavailable: {error}"));
    }
    if options.llm_mode == EvalLlmMode::Enabled {
        warnings.push(
            "LLM mode was requested, but EVAL-001 only records deterministic context; generation must be supplied by CONTEXT-002"
                .to_string(),
        );
    }

    let mut report = EvalRunReport {
        schema_version: EVAL_SCHEMA_VERSION,
        run_id,
        opticcode_version: env!("CARGO_PKG_VERSION").to_string(),
        git_commit: discover_git_commit(suite_dir),
        suite_id: suite.id,
        suite_version: suite.version,
        configuration,
        configuration_hash,
        rag_identity,
        started_at_unix_ms,
        duration_us: duration_us(started.elapsed()),
        environment: EvalEnvironment {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            family: std::env::consts::FAMILY.to_string(),
            logical_cpus: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
        },
        results,
        summary,
        baseline: None,
        warnings,
    };
    if let Some(baseline) = options.baseline.as_ref() {
        report.baseline = Some(compare_reports(baseline, &report));
    }
    Ok(report)
}

fn run_case_strategy(
    case: &EvalCase,
    root: &Path,
    strategy: EvalStrategy,
    repetition: u32,
    configuration: &EvalConfiguration,
    rag: RagCaseInput<'_>,
) -> EvalCaseResult {
    let result = match strategy {
        EvalStrategy::Legacy => run_legacy(case, root, configuration),
        EvalStrategy::Symbol => run_symbol(case, root),
        EvalStrategy::Exact => run_exact(case, root, configuration.context.exact_limit),
        EvalStrategy::Rag => run_rag(case, rag.index, configuration.rag.limit, rag.error),
    };
    match result {
        Ok(Some(execution)) => completed_result(case, strategy, repetition, execution),
        Ok(None) => skipped_result(case, strategy, repetition, "RAG index was not configured"),
        Err(error) => failed_result(case, strategy, repetition, sanitize_error(root, &error)),
    }
}

struct StrategyExecution {
    observed: EvalObserved,
    context: EvalContextMetrics,
}

#[derive(Debug, Clone, Copy)]
struct RagCaseInput<'a> {
    index: Option<&'a Path>,
    error: Option<&'a str>,
}

fn run_legacy(
    case: &EvalCase,
    root: &Path,
    configuration: &EvalConfiguration,
) -> Result<Option<StrategyExecution>> {
    let started = Instant::now();
    let max_files = case
        .context_budget
        .max_files
        .min(configuration.context.legacy_max_files);
    let context = build_project_context_with_limits(
        root,
        max_files,
        configuration.context.legacy_max_bytes_per_file,
        case.context_budget.max_chars,
    )?;
    let prompt_context = context.to_prompt_context();
    let items = context
        .snippets
        .iter()
        .enumerate()
        .map(|(index, snippet)| EvalRetrievedItem {
            rank: index + 1,
            path: portable_path(&snippet.path.to_string_lossy()),
            symbol: None,
            score: None,
            source: "legacy_file_priority_v1".to_string(),
            bytes: snippet.content.len(),
            chars: snippet.content.chars().count(),
            estimated_tokens: estimate_tokens(&snippet.content),
            truncated: snippet.truncated,
            content_hash: Some(
                blake3::hash(snippet.content.as_bytes())
                    .to_hex()
                    .to_string(),
            ),
        })
        .collect::<Vec<_>>();
    let total_us = duration_us(started.elapsed());
    let analysis_complete = items.iter().all(|item| !item.truncated);
    Ok(Some(build_execution(
        case,
        items,
        prompt_context.len(),
        prompt_context.chars().count(),
        analysis_complete,
        !analysis_complete || context.report.sampled_files.len() > context.snippets.len(),
        total_us,
        0,
        0,
    )))
}

fn run_symbol(case: &EvalCase, root: &Path) -> Result<Option<StrategyExecution>> {
    let options = JavaTaskContextOptions {
        max_snippets: case
            .context_budget
            .max_snippets
            .min(case.context_budget.max_files),
        max_context_chars: case.context_budget.max_chars,
        max_context_bytes: case.context_budget.max_chars.saturating_mul(4),
        max_estimated_tokens: case.context_budget.max_estimated_tokens,
        ..JavaTaskContextOptions::default()
    };
    let report = build_java_task_context(root, &case.prompt, options)?;
    let items = report
        .snippets
        .iter()
        .enumerate()
        .map(|(index, snippet)| EvalRetrievedItem {
            rank: index + 1,
            path: portable_path(&snippet.file.to_string_lossy()),
            symbol: snippet.symbol_id.clone(),
            score: Some(f64::from(snippet.score)),
            source: format!("symbol:{}", snippet.role.as_str()),
            bytes: snippet.content_bytes,
            chars: snippet.content_chars,
            estimated_tokens: snippet.estimated_tokens,
            truncated: snippet.truncated,
            content_hash: Some(snippet.content_hash.clone()),
        })
        .collect::<Vec<_>>();
    let mut execution = build_execution(
        case,
        items,
        report.budget.rendered_bytes,
        report.budget.rendered_chars,
        report.analysis_complete,
        report.truncated,
        report.timings.total_us,
        report.timings.ranking_us,
        report.timings.snippets_us,
    );
    execution
        .observed
        .selected_symbols
        .extend(report.primary_symbols);
    execution.observed.selected_symbols.sort();
    execution.observed.selected_symbols.dedup();
    execution.observed.warnings = report.warnings;
    execution.context.discovery_us = report
        .timings
        .index_us
        .saturating_add(report.timings.query_us);
    Ok(Some(execution))
}

fn run_exact(
    case: &EvalCase,
    root: &Path,
    configured_limit: usize,
) -> Result<Option<StrategyExecution>> {
    let started = Instant::now();
    let query = derive_exact_query(case);
    let limit = configured_limit.min(case.context_budget.max_snippets);
    let hits = search_workspace(root, &query, limit)?;
    let items = hits
        .into_iter()
        .enumerate()
        .map(|(index, hit)| EvalRetrievedItem {
            rank: index + 1,
            path: portable_path(&hit.path.to_string_lossy()),
            symbol: None,
            score: None,
            source: format!("exact_line:{}", hit.line_number),
            bytes: hit.line.len(),
            chars: hit.line.chars().count(),
            estimated_tokens: estimate_tokens(&hit.line),
            truncated: false,
            content_hash: Some(blake3::hash(hit.line.as_bytes()).to_hex().to_string()),
        })
        .collect::<Vec<_>>();
    let bytes = items.iter().map(|item| item.bytes).sum();
    let chars = items.iter().map(|item| item.chars).sum();
    let total_us = duration_us(started.elapsed());
    Ok(Some(build_execution(
        case, items, bytes, chars, true, false, total_us, 0, 0,
    )))
}

fn run_rag(
    case: &EvalCase,
    rag_index: Option<&Path>,
    limit: usize,
    rag_error: Option<&str>,
) -> Result<Option<StrategyExecution>> {
    let Some(rag_index) = rag_index else {
        return Ok(None);
    };
    if let Some(error) = rag_error {
        bail!("RAG index validation failed before evaluation: {error}");
    }
    let report = search_rag_index_report(
        rag_index,
        &case.prompt,
        limit.min(case.context_budget.max_snippets),
    )?;
    let items = report
        .hits
        .into_iter()
        .enumerate()
        .map(|(index, hit)| EvalRetrievedItem {
            rank: index + 1,
            path: portable_path(&hit.document_path),
            symbol: None,
            score: Some(hit.score as f64),
            source: format!("rag_v2:{}", hit.chunk_id),
            bytes: hit.preview.len(),
            chars: hit.preview.chars().count(),
            estimated_tokens: estimate_tokens(&hit.preview),
            truncated: true,
            content_hash: Some(blake3::hash(hit.preview.as_bytes()).to_hex().to_string()),
        })
        .collect::<Vec<_>>();
    let bytes = items.iter().map(|item| item.bytes).sum();
    let chars = items.iter().map(|item| item.chars).sum();
    Ok(Some(build_execution(
        case,
        items,
        bytes,
        chars,
        true,
        false,
        report.duration_us,
        report.duration_us,
        0,
    )))
}

#[allow(clippy::too_many_arguments)]
fn build_execution(
    case: &EvalCase,
    items: Vec<EvalRetrievedItem>,
    rendered_bytes: usize,
    rendered_chars: usize,
    analysis_complete: bool,
    budget_reached: bool,
    total_us: u64,
    ranking_us: u64,
    materialization_us: u64,
) -> StrategyExecution {
    let selected_files = items
        .iter()
        .map(|item| item.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let selected_symbols = items
        .iter()
        .filter_map(|item| item.symbol.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let expected_files_present =
        count_matching_paths(&selected_files, &case.expected.relevant_files);
    let irrelevant_files_present =
        count_matching_paths(&selected_files, &case.expected.irrelevant_files);
    let expected_symbols_present =
        count_matching_symbols(&selected_symbols, &case.expected.relevant_symbols);
    let snippet_count = items.len();
    let truncated_snippets = items.iter().filter(|item| item.truncated).count();
    StrategyExecution {
        observed: EvalObserved {
            retrieved: items,
            selected_files: selected_files.clone(),
            selected_symbols,
            warnings: Vec::new(),
        },
        context: EvalContextMetrics {
            files: selected_files.len(),
            snippets: snippet_count,
            chars: rendered_chars,
            bytes: rendered_bytes,
            estimated_tokens: estimate_tokens_chars(rendered_chars),
            token_estimator: "estimate:ceil_unicode_chars_div_4".to_string(),
            actual_prompt_tokens: None,
            generated_tokens: None,
            expected_files_present,
            expected_symbols_present,
            irrelevant_files_present,
            truncated_snippets,
            budget_reached,
            analysis_complete,
            discovery_us: total_us
                .saturating_sub(ranking_us)
                .saturating_sub(materialization_us),
            ranking_us,
            materialization_us,
            total_us,
        },
    }
}

fn completed_result(
    case: &EvalCase,
    strategy: EvalStrategy,
    repetition: u32,
    mut execution: StrategyExecution,
) -> EvalCaseResult {
    execution.context.snippets = execution.observed.retrieved.len();
    let retrieval = calculate_retrieval_metrics(&case.expected, &execution.observed.retrieved);
    EvalCaseResult {
        case_id: case.id.clone(),
        category: case.category,
        fixture: case.fixture.clone(),
        strategy,
        repetition,
        status: EvalCaseStatus::Completed,
        skip_reason: None,
        error: None,
        observed: execution.observed,
        metrics: EvalCaseMetrics {
            retrieval,
            context: execution.context,
            response: response_metrics(&case.expected, false),
            generation: None,
        },
        response: None,
    }
}

fn skipped_result(
    case: &EvalCase,
    strategy: EvalStrategy,
    repetition: u32,
    reason: &str,
) -> EvalCaseResult {
    empty_result(
        case,
        strategy,
        repetition,
        EvalCaseStatus::Skipped,
        Some(reason.to_string()),
        None,
    )
}

fn failed_result(
    case: &EvalCase,
    strategy: EvalStrategy,
    repetition: u32,
    error: String,
) -> EvalCaseResult {
    empty_result(
        case,
        strategy,
        repetition,
        EvalCaseStatus::Failed,
        None,
        Some(error),
    )
}

fn empty_result(
    case: &EvalCase,
    strategy: EvalStrategy,
    repetition: u32,
    status: EvalCaseStatus,
    skip_reason: Option<String>,
    error: Option<String>,
) -> EvalCaseResult {
    EvalCaseResult {
        case_id: case.id.clone(),
        category: case.category,
        fixture: case.fixture.clone(),
        strategy,
        repetition,
        status,
        skip_reason,
        error,
        observed: EvalObserved::default(),
        metrics: EvalCaseMetrics {
            retrieval: calculate_retrieval_metrics(&case.expected, &[]),
            context: EvalContextMetrics {
                token_estimator: "estimate:ceil_unicode_chars_div_4".to_string(),
                ..EvalContextMetrics::default()
            },
            response: response_metrics(&case.expected, false),
            generation: None,
        },
        response: None,
    }
}

fn response_metrics(expected: &EvalExpected, generated: bool) -> EvalResponseMetrics {
    EvalResponseMetrics {
        generated,
        expected_facts_total: expected.facts.len(),
        scope_preserved: true,
        human_review: if generated {
            EvalHumanReview::PendingHumanReview
        } else {
            EvalHumanReview::NotRequired
        },
        ..EvalResponseMetrics::default()
    }
}

fn resolve_fixtures(
    suite: &EvalSuite,
    suite_dir: &Path,
    external: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, ResolvedFixture>> {
    let mut resolved = BTreeMap::new();
    for (name, fixture) in &suite.fixtures {
        let value = match fixture {
            super::EvalFixture::Versioned { path } => {
                let candidate = suite_dir.join(path);
                let canonical = fs::canonicalize(&candidate).with_context(|| {
                    format!(
                        "versioned evaluation fixture `{name}` is unavailable: {}",
                        candidate.display()
                    )
                })?;
                if !canonical.is_dir() {
                    bail!("evaluation fixture `{name}` is not a directory");
                }
                ResolvedFixture::Available(canonical)
            }
            super::EvalFixture::External { external_id, .. } => {
                if let Some(path) = external.get(external_id) {
                    match fs::canonicalize(path) {
                        Ok(path) if path.is_dir() => ResolvedFixture::Available(path),
                        Ok(_) => ResolvedFixture::MissingExternal(format!(
                            "external fixture `{external_id}` is not a directory"
                        )),
                        Err(_) => ResolvedFixture::MissingExternal(format!(
                            "external fixture `{external_id}` is unavailable"
                        )),
                    }
                } else {
                    ResolvedFixture::MissingExternal(format!(
                        "external fixture `{external_id}` was not provided"
                    ))
                }
            }
        };
        resolved.insert(name.clone(), value);
    }
    Ok(resolved)
}

fn capture_fixture_fingerprints(
    fixtures: &BTreeMap<String, ResolvedFixture>,
) -> Result<BTreeMap<String, String>> {
    let mut fingerprints = BTreeMap::new();
    for (name, fixture) in fixtures {
        if let ResolvedFixture::Available(root) = fixture {
            fingerprints.insert(name.clone(), fixture_fingerprint(root)?);
        }
    }
    Ok(fingerprints)
}

fn verify_fixture_fingerprints(
    fixtures: &BTreeMap<String, ResolvedFixture>,
    before: &BTreeMap<String, String>,
) -> Result<()> {
    for (name, expected) in before {
        let Some(ResolvedFixture::Available(root)) = fixtures.get(name) else {
            bail!("evaluation fixture `{name}` disappeared before integrity verification");
        };
        let actual = fixture_fingerprint(root)?;
        if &actual != expected {
            bail!(
                "evaluation fixture `{name}` changed during a read-only run; report publication refused"
            );
        }
    }
    Ok(())
}

fn fixture_fingerprint(root: &Path) -> Result<String> {
    let mut files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_fingerprint_entry)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    files.retain(|entry| entry.file_type().is_file());
    if files.len() > MAX_FINGERPRINT_FILES {
        bail!(
            "evaluation fixture exceeds read-only fingerprint limit of {MAX_FINGERPRINT_FILES} files"
        );
    }
    files.sort_by(|left, right| left.path().cmp(right.path()));
    let mut hasher = blake3::Hasher::new();
    let mut bytes_seen = 0u64;
    for entry in files {
        let relative = entry.path().strip_prefix(root)?;
        let bytes = fs::read(entry.path()).with_context(|| {
            format!(
                "failed to fingerprint evaluation fixture file: {}",
                entry.path().display()
            )
        })?;
        bytes_seen = bytes_seen.saturating_add(bytes.len() as u64);
        if bytes_seen > MAX_FINGERPRINT_BYTES {
            bail!(
                "evaluation fixture exceeds read-only fingerprint byte limit of {MAX_FINGERPRINT_BYTES}"
            );
        }
        hasher.update(normalize_path(&relative.to_string_lossy()).as_bytes());
        hasher.update(&[0]);
        hasher.update(&bytes);
        hasher.update(&[0xff]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn evaluation_fixture_fingerprint(root: &Path) -> Result<String> {
    fixture_fingerprint(root)
}

fn should_fingerprint_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    if entry.path_is_symlink() {
        return false;
    }
    !matches!(
        entry.file_name().to_string_lossy().as_ref(),
        ".git"
            | ".opticcode"
            | "target"
            | "build"
            | ".gradle"
            | ".idea"
            | ".vscode"
            | "node_modules"
    )
}

fn resolve_rag_identity(index: Option<&Path>) -> (Option<EvalRagIdentity>, Option<String>) {
    let Some(index) = index else {
        return (None, None);
    };
    match load_active_rag_manifest(index) {
        Ok(manifest) => {
            let bytes = serde_json::to_vec(&manifest).unwrap_or_default();
            (
                Some(EvalRagIdentity {
                    schema_version: manifest.schema_version,
                    generation_id: manifest.generation_id,
                    configuration_hash: manifest.configuration_hash,
                    manifest_blake3: blake3::hash(&bytes).to_hex().to_string(),
                }),
                None,
            )
        }
        Err(error) => (None, Some(error.to_string())),
    }
}

fn validate_run_options(options: &EvalRunOptions) -> Result<()> {
    if options.strategies.is_empty() {
        bail!("at least one evaluation strategy is required");
    }
    if options.repetitions == 0 || options.repetitions > MAX_EVAL_REPETITIONS {
        bail!("evaluation repetitions must be between 1 and {MAX_EVAL_REPETITIONS}");
    }
    if options.case_limit == Some(0) {
        bail!("evaluation case limit must be greater than zero");
    }
    if options
        .case_ids
        .iter()
        .any(|case_id| case_id.trim().is_empty())
    {
        bail!("evaluation case ids must not be empty");
    }
    if options.case_ids.iter().collect::<BTreeSet<_>>().len() != options.case_ids.len() {
        bail!("evaluation case ids must not contain duplicates");
    }
    if options.rag_limit == 0 || options.rag_limit > 1_000 {
        bail!("evaluation RAG limit must be between 1 and 1000");
    }
    if options.llm_mode == EvalLlmMode::Enabled && options.generation.is_none() {
        bail!("LLM evaluation mode requires generation configuration");
    }
    Ok(())
}

fn configuration_hash(suite: &EvalSuite, configuration: &EvalConfiguration) -> Result<String> {
    #[derive(Serialize)]
    struct HashInput<'a> {
        suite_id: &'a str,
        suite_version: &'a str,
        configuration: &'a EvalConfiguration,
    }
    let encoded = serde_json::to_vec(&HashInput {
        suite_id: &suite.id,
        suite_version: &suite.version,
        configuration,
    })?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

fn discover_git_commit(start: &Path) -> Option<String> {
    let repository = start.ancestors().find(|path| path.join(".git").exists())?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    (!commit.is_empty()).then(|| commit.to_string())
}

fn derive_exact_query(case: &EvalCase) -> String {
    if let Some(query) = case.exact_query.as_deref() {
        return query.to_string();
    }
    if let Some(symbol) = case.expected.relevant_symbols.first() {
        let member = symbol.rsplit('#').next().unwrap_or(symbol);
        return member
            .split('(')
            .next()
            .unwrap_or(member)
            .rsplit('.')
            .next()
            .unwrap_or(member)
            .to_string();
    }
    case.prompt.clone()
}

fn count_matching_paths(actual: &[String], expected: &[String]) -> usize {
    expected
        .iter()
        .filter(|expected| {
            let expected = normalize_path(expected);
            actual.iter().any(|actual| {
                let actual = normalize_path(actual);
                actual == expected || actual.ends_with(&format!("/{expected}"))
            })
        })
        .count()
}

fn count_matching_symbols(actual: &[String], expected: &[String]) -> usize {
    expected
        .iter()
        .filter(|expected| {
            actual
                .iter()
                .any(|actual| actual.eq_ignore_ascii_case(expected))
        })
        .count()
}

fn reject_duplicates(values: &[String], label: &str) -> Result<()> {
    let mut unique = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            bail!("{label} contains an empty value");
        }
        if !unique.insert(value.trim().to_ascii_lowercase()) {
            bail!("{label} contains duplicate value `{value}`");
        }
    }
    Ok(())
}

fn sanitize_error(root: &Path, error: &anyhow::Error) -> String {
    error
        .to_string()
        .replace(&root.to_string_lossy().to_string(), "<fixture>")
}

fn portable_path(value: &str) -> String {
    value.trim().replace('\\', "/")
}

fn estimate_tokens(value: &str) -> usize {
    estimate_tokens_chars(value.chars().count())
}

fn estimate_tokens_chars(chars: usize) -> usize {
    chars.saturating_add(3) / 4
}

fn unix_time_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_millis()
        .min(u128::from(u64::MAX)) as u64)
}

fn duration_us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}
