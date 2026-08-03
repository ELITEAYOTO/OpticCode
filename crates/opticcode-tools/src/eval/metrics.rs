use std::collections::{BTreeMap, BTreeSet};

use super::{
    EvalBaselineComparison, EvalCaseResult, EvalCaseStatus, EvalExpected, EvalLlmGenerationStatus,
    EvalRegression, EvalRegressionSeverity, EvalRetrievalMetrics, EvalRunReport, EvalStrategy,
    EvalStrategySummary, EvalSummary,
};

pub fn calculate_retrieval_metrics(
    expected: &EvalExpected,
    retrieved: &[super::EvalRetrievedItem],
) -> EvalRetrievalMetrics {
    let relevant_files = expected
        .relevant_files
        .iter()
        .map(|value| normalize_path(value))
        .collect::<BTreeSet<_>>();
    // Files are the canonical retrieval unit when supplied. Symbols remain a
    // fallback for symbol-only cases, avoiding double-counting one snippet as
    // both its file and declaration in Recall/NDCG.
    let relevant_symbols = if relevant_files.is_empty() {
        expected
            .relevant_symbols
            .iter()
            .map(|value| normalize_symbol(value))
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let relevant_expected = relevant_files.len().saturating_add(relevant_symbols.len());

    let mut identities = BTreeSet::new();
    let mut unique_files = BTreeSet::new();
    let mut matched_units = BTreeSet::new();
    let mut matched_by_rank = Vec::with_capacity(retrieved.len());
    let mut duplicates = 0usize;
    let mut out_of_scope_results = 0usize;

    for item in retrieved {
        let path = normalize_path(&item.path);
        let symbol = item.symbol.as_deref().map(normalize_symbol);
        let identity = format!(
            "{}\u{0}{}\u{0}{}",
            path,
            symbol.as_deref().unwrap_or_default(),
            item.source
        );
        if !identities.insert(identity) {
            duplicates = duplicates.saturating_add(1);
            matched_by_rank.push(BTreeSet::new());
            continue;
        }
        unique_files.insert(path.clone());

        let mut item_matches = BTreeSet::new();
        for expected_file in &relevant_files {
            if path_matches(&path, expected_file) {
                item_matches.insert(format!("file:{expected_file}"));
            }
        }
        if let Some(symbol) = symbol {
            for expected_symbol in &relevant_symbols {
                if symbol_matches(&symbol, expected_symbol) {
                    item_matches.insert(format!("symbol:{expected_symbol}"));
                }
            }
        }
        if item_matches.is_empty() && relevant_expected > 0 {
            out_of_scope_results = out_of_scope_results.saturating_add(1);
        }
        matched_units.extend(item_matches.iter().cloned());
        matched_by_rank.push(item_matches);
    }

    let hit_at = |limit: usize| {
        matched_by_rank
            .iter()
            .take(limit)
            .any(|matches| !matches.is_empty())
    };
    let recall_at = |limit: usize| {
        if relevant_expected == 0 {
            return None;
        }
        let found = matched_by_rank
            .iter()
            .take(limit)
            .flat_map(|matches| matches.iter().cloned())
            .collect::<BTreeSet<_>>()
            .len();
        Some(found as f64 / relevant_expected as f64)
    };

    let first_relevant_rank = matched_by_rank
        .iter()
        .position(|matches| !matches.is_empty())
        .map(|index| index + 1);
    let reciprocal_rank = first_relevant_rank.map(|rank| 1.0 / rank as f64);
    let ndcg_at_5 = (relevant_expected > 0).then(|| {
        let mut credited = BTreeSet::new();
        let dcg = matched_by_rank
            .iter()
            .take(5)
            .enumerate()
            .map(|(index, matches)| {
                let new_relevant = matches.iter().any(|unit| credited.insert(unit.clone()));
                if new_relevant {
                    1.0 / ((index + 2) as f64).log2()
                } else {
                    0.0
                }
            })
            .sum::<f64>();
        let ideal = (0..relevant_expected.min(5))
            .map(|index| 1.0 / ((index + 2) as f64).log2())
            .sum::<f64>();
        if ideal == 0.0 {
            0.0
        } else {
            dcg / ideal
        }
    });
    let unique_result_count = retrieved.len().saturating_sub(duplicates);

    EvalRetrievalMetrics {
        hit_at_1: (relevant_expected > 0).then(|| hit_at(1)),
        hit_at_3: (relevant_expected > 0).then(|| hit_at(3)),
        hit_at_5: (relevant_expected > 0).then(|| hit_at(5)),
        recall_at_1: recall_at(1),
        recall_at_3: recall_at(3),
        recall_at_5: recall_at(5),
        recall_at_k: recall_at(retrieved.len()),
        reciprocal_rank,
        ndcg_at_5,
        first_relevant_rank,
        relevant_expected,
        relevant_found_at_k: matched_units.len(),
        duplicates,
        unique_files: unique_files.len(),
        file_diversity: if unique_result_count == 0 {
            0.0
        } else {
            unique_files.len() as f64 / unique_result_count as f64
        },
        out_of_scope_results,
        result_count: retrieved.len(),
    }
}

pub fn summarize_results(case_count: usize, results: &[EvalCaseResult]) -> EvalSummary {
    let mut by_strategy = BTreeMap::<EvalStrategy, Vec<&EvalCaseResult>>::new();
    for result in results {
        by_strategy.entry(result.strategy).or_default().push(result);
    }

    let strategies = by_strategy
        .into_iter()
        .map(|(strategy, strategy_results)| summarize_strategy(strategy, &strategy_results))
        .collect::<Vec<_>>();

    EvalSummary {
        case_count,
        execution_count: results.len(),
        completed: results
            .iter()
            .filter(|result| result.status == EvalCaseStatus::Completed)
            .count(),
        skipped: results
            .iter()
            .filter(|result| result.status == EvalCaseStatus::Skipped)
            .count(),
        failed: results
            .iter()
            .filter(|result| result.status == EvalCaseStatus::Failed)
            .count(),
        strategies,
    }
}

pub fn compare_reports(
    baseline: &EvalRunReport,
    candidate: &EvalRunReport,
) -> EvalBaselineComparison {
    let comparable = baseline.suite_id == candidate.suite_id
        && baseline.suite_version == candidate.suite_version;
    let mut comparison = EvalBaselineComparison {
        baseline_run_id: baseline.run_id.clone(),
        candidate_run_id: candidate.run_id.clone(),
        comparable,
        regressions: Vec::new(),
        improvements: Vec::new(),
    };
    if !comparable {
        return comparison;
    }

    let baseline_by_strategy = baseline
        .summary
        .strategies
        .iter()
        .map(|summary| (summary.strategy, summary))
        .collect::<BTreeMap<_, _>>();
    for current in &candidate.summary.strategies {
        let Some(previous) = baseline_by_strategy.get(&current.strategy) else {
            continue;
        };
        compare_higher_is_better(
            &mut comparison,
            current.strategy,
            "hit_at_5",
            previous.hit_at_5,
            current.hit_at_5,
            0.01,
        );
        compare_higher_is_better(
            &mut comparison,
            current.strategy,
            "mean_recall_at_k",
            previous.mean_recall_at_k,
            current.mean_recall_at_k,
            0.01,
        );
        compare_higher_is_better(
            &mut comparison,
            current.strategy,
            "mean_reciprocal_rank",
            previous.mean_reciprocal_rank,
            current.mean_reciprocal_rank,
            0.01,
        );
        compare_lower_is_better(
            &mut comparison,
            current.strategy,
            "mean_estimated_tokens",
            previous.mean_estimated_tokens,
            current.mean_estimated_tokens,
            0.05,
        );
        compare_lower_is_better(
            &mut comparison,
            current.strategy,
            "latency_p95_us",
            previous.latency_p95_us as f64,
            current.latency_p95_us as f64,
            0.20,
        );
    }
    comparison
}

pub fn percentile(values: &mut [u64], percentile: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let rank = (percentile.clamp(0.0, 1.0) * values.len() as f64).ceil() as usize;
    values[rank.saturating_sub(1).min(values.len() - 1)]
}

fn summarize_strategy(strategy: EvalStrategy, results: &[&EvalCaseResult]) -> EvalStrategySummary {
    let completed = results
        .iter()
        .filter(|result| result.status == EvalCaseStatus::Completed)
        .copied()
        .collect::<Vec<_>>();
    let mut latencies = completed
        .iter()
        .map(|result| result.metrics.context.total_us)
        .collect::<Vec<_>>();
    let latency_p50_us = percentile(&mut latencies, 0.50);
    let latency_p95_us = percentile(&mut latencies, 0.95);
    let generations = completed
        .iter()
        .filter_map(|result| result.metrics.generation.as_ref())
        .collect::<Vec<_>>();
    let generated = generations
        .iter()
        .filter(|generation| generation.status == EvalLlmGenerationStatus::Generated)
        .copied()
        .collect::<Vec<_>>();
    let mut generation_latencies = generated
        .iter()
        .filter_map(|generation| generation.client_total_us)
        .collect::<Vec<_>>();
    let generation_latency_p50_us =
        (!generation_latencies.is_empty()).then(|| percentile(&mut generation_latencies, 0.50));
    let generation_latency_p95_us =
        (!generation_latencies.is_empty()).then(|| percentile(&mut generation_latencies, 0.95));

    EvalStrategySummary {
        strategy,
        completed: completed.len(),
        skipped: results
            .iter()
            .filter(|result| result.status == EvalCaseStatus::Skipped)
            .count(),
        failed: results
            .iter()
            .filter(|result| result.status == EvalCaseStatus::Failed)
            .count(),
        hit_at_1: mean_bool(
            completed
                .iter()
                .filter_map(|result| result.metrics.retrieval.hit_at_1),
        ),
        hit_at_3: mean_bool(
            completed
                .iter()
                .filter_map(|result| result.metrics.retrieval.hit_at_3),
        ),
        hit_at_5: mean_bool(
            completed
                .iter()
                .filter_map(|result| result.metrics.retrieval.hit_at_5),
        ),
        mean_recall_at_k: mean(
            completed
                .iter()
                .filter_map(|result| result.metrics.retrieval.recall_at_k),
        ),
        mean_reciprocal_rank: mean(
            completed
                .iter()
                .filter_map(|result| result.metrics.retrieval.reciprocal_rank),
        ),
        mean_ndcg_at_5: mean(
            completed
                .iter()
                .filter_map(|result| result.metrics.retrieval.ndcg_at_5),
        ),
        mean_estimated_tokens: mean_or_zero(
            completed
                .iter()
                .map(|result| result.metrics.context.estimated_tokens as f64),
        ),
        latency_p50_us,
        latency_p95_us,
        analysis_complete_rate: mean_or_zero(
            completed
                .iter()
                .map(|result| f64::from(result.metrics.context.analysis_complete)),
        ),
        generated_responses: generated.len(),
        generation_skipped: generations
            .iter()
            .filter(|generation| generation.status == EvalLlmGenerationStatus::Skipped)
            .count(),
        generation_failed: generations
            .iter()
            .filter(|generation| generation.status == EvalLlmGenerationStatus::Failed)
            .count(),
        mean_actual_prompt_tokens: mean(
            generated.iter().filter_map(|generation| {
                generation.actual_prompt_tokens.map(|tokens| tokens as f64)
            }),
        ),
        mean_generated_tokens: mean(
            generated
                .iter()
                .filter_map(|generation| generation.generated_tokens.map(|tokens| tokens as f64)),
        ),
        mean_deterministic_quality: mean(
            completed
                .iter()
                .filter_map(|result| result.metrics.response.deterministic_quality_score),
        ),
        generation_latency_p50_us,
        generation_latency_p95_us,
    }
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values = values.collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn mean_or_zero(values: impl Iterator<Item = f64>) -> f64 {
    mean(values).unwrap_or(0.0)
}

fn mean_bool(values: impl Iterator<Item = bool>) -> Option<f64> {
    mean(values.map(f64::from))
}

fn compare_higher_is_better(
    comparison: &mut EvalBaselineComparison,
    strategy: EvalStrategy,
    metric: &str,
    baseline: Option<f64>,
    candidate: Option<f64>,
    threshold: f64,
) {
    let (Some(baseline), Some(candidate)) = (baseline, candidate) else {
        return;
    };
    compare_metric(
        comparison, strategy, metric, baseline, candidate, threshold, true,
    );
}

fn compare_lower_is_better(
    comparison: &mut EvalBaselineComparison,
    strategy: EvalStrategy,
    metric: &str,
    baseline: f64,
    candidate: f64,
    threshold: f64,
) {
    compare_metric(
        comparison, strategy, metric, baseline, candidate, threshold, false,
    );
}

#[allow(clippy::too_many_arguments)]
fn compare_metric(
    comparison: &mut EvalBaselineComparison,
    strategy: EvalStrategy,
    metric: &str,
    baseline: f64,
    candidate: f64,
    threshold: f64,
    higher_is_better: bool,
) {
    let delta = candidate - baseline;
    let relative = if baseline.abs() < f64::EPSILON {
        delta
    } else {
        delta / baseline.abs()
    };
    let regressed = if higher_is_better {
        delta < -threshold
    } else {
        relative > threshold
    };
    let improved = if higher_is_better {
        delta > threshold
    } else {
        relative < -threshold
    };
    if !regressed && !improved {
        return;
    }
    let severity = if regressed && relative.abs() >= threshold * 2.0 {
        EvalRegressionSeverity::Critical
    } else if regressed {
        EvalRegressionSeverity::Warning
    } else {
        EvalRegressionSeverity::Info
    };
    let entry = EvalRegression {
        strategy,
        metric: metric.to_string(),
        baseline,
        candidate,
        delta,
        severity,
        reason: if regressed {
            format!("{metric} regressed beyond the configured tolerance")
        } else {
            format!("{metric} improved beyond the configured tolerance")
        },
    };
    if regressed {
        comparison.regressions.push(entry);
    } else {
        comparison.improvements.push(entry);
    }
}

pub(crate) fn normalize_path(value: &str) -> String {
    let mut normalized = value.trim().replace('\\', "/").to_ascii_lowercase();
    while normalized.starts_with("./") {
        normalized = normalized[2..].to_string();
    }
    if let Some(colon) = normalized.find(':') {
        if colon > 1 && !normalized[..colon].contains('/') {
            normalized = normalized[colon + 1..].trim_start_matches('/').to_string();
        }
    }
    normalized
}

fn normalize_symbol(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn path_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual.ends_with(&format!("/{expected}"))
}

fn symbol_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual.ends_with(expected)
}
