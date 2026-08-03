use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use super::{EvalBaselineComparison, EvalRunReport};

#[derive(Debug, Clone, Serialize)]
pub struct EvalReportPaths {
    pub json: PathBuf,
    pub markdown: PathBuf,
}

pub fn load_eval_report(path: &Path) -> Result<EvalRunReport> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read evaluation report: {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse evaluation report: {}", path.display()))
}

pub fn write_eval_reports(report: &EvalRunReport, output_dir: &Path) -> Result<EvalReportPaths> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create evaluation report directory: {}",
            output_dir.display()
        )
    })?;
    let stem = format!("eval-{}", report.run_id);
    let json = output_dir.join(format!("{stem}.json"));
    let markdown = output_dir.join(format!("{stem}.md"));
    write_atomic(&json, &serde_json::to_vec_pretty(report)?)?;
    write_atomic(&markdown, render_markdown_report(report).as_bytes())?;
    Ok(EvalReportPaths { json, markdown })
}

pub fn render_markdown_report(report: &EvalRunReport) -> String {
    let mut output = String::new();
    output.push_str(&format!("# OpticCode evaluation `{}`\n\n", report.run_id));
    output.push_str(&format!(
        "Suite: `{}` v`{}`  \nConfiguration: `{}`  \nExecutions: {} completed, {} skipped, {} failed  \nDuration: {:.3} ms\n\n",
        report.suite_id,
        report.suite_version,
        report.configuration_hash,
        report.summary.completed,
        report.summary.skipped,
        report.summary.failed,
        report.duration_us as f64 / 1_000.0,
    ));
    output.push_str("## Strategy comparison\n\n");
    output.push_str(
        "| Strategy | Done | Skip | Fail | Hit@1 | Hit@3 | Hit@5 | Recall@k | MRR | NDCG@5 | Est. tokens | p50 ms | p95 ms | Complete |\n",
    );
    output.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for summary in &report.summary.strategies {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.1} | {:.3} | {:.3} | {:.1}% |\n",
            summary.strategy,
            summary.completed,
            summary.skipped,
            summary.failed,
            format_rate(summary.hit_at_1),
            format_rate(summary.hit_at_3),
            format_rate(summary.hit_at_5),
            format_rate(summary.mean_recall_at_k),
            format_rate(summary.mean_reciprocal_rank),
            format_rate(summary.mean_ndcg_at_5),
            summary.mean_estimated_tokens,
            summary.latency_p50_us as f64 / 1_000.0,
            summary.latency_p95_us as f64 / 1_000.0,
            summary.analysis_complete_rate * 100.0,
        ));
    }

    if let Some(comparison) = &report.baseline {
        render_baseline(&mut output, comparison);
    }

    output.push_str("\n## Cases\n\n");
    output.push_str("| Case | Category | Strategy | Run | Status | Hit@5 | Recall@k | MRR | Files | Snippets | Est. tokens | Time ms |\n");
    output.push_str("|---|---|---|---:|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for result in &report.results {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {:?} | {} | {} | {} | {} | {} | {} | {:.3} |\n",
            escape_table(&result.case_id),
            result.category.as_str(),
            result.strategy,
            result.repetition,
            result.status,
            format_rate(result.metrics.retrieval.hit_at_5.map(f64::from)),
            format_rate(result.metrics.retrieval.recall_at_k),
            format_rate(result.metrics.retrieval.reciprocal_rank),
            result.metrics.context.files,
            result.metrics.context.snippets,
            result.metrics.context.estimated_tokens,
            result.metrics.context.total_us as f64 / 1_000.0,
        ));
    }

    if !report.warnings.is_empty() {
        output.push_str("\n## Warnings\n\n");
        for warning in &report.warnings {
            output.push_str(&format!("- {}\n", warning.replace('\n', " ")));
        }
    }

    output.push_str("\n## Exact configuration\n\n```json\n");
    output.push_str(
        &serde_json::to_string_pretty(&report.configuration)
            .unwrap_or_else(|_| "{\"error\":\"configuration serialization failed\"}".to_string()),
    );
    output.push_str("\n```\n");
    output
}

fn render_baseline(output: &mut String, comparison: &EvalBaselineComparison) {
    output.push_str("\n## Baseline comparison\n\n");
    output.push_str(&format!(
        "Baseline: `{}`  \nComparable: `{}`\n\n",
        comparison.baseline_run_id, comparison.comparable
    ));
    if comparison.regressions.is_empty() {
        output.push_str("No regression crossed the configured tolerance.\n");
    } else {
        output.push_str("### Regressions\n\n");
        for regression in &comparison.regressions {
            output.push_str(&format!(
                "- `{}` / `{}`: {:.4} -> {:.4} ({:+.4}, {:?})\n",
                regression.strategy,
                regression.metric,
                regression.baseline,
                regression.candidate,
                regression.delta,
                regression.severity,
            ));
        }
    }
    if !comparison.improvements.is_empty() {
        output.push_str("\n### Improvements\n\n");
        for improvement in &comparison.improvements {
            output.push_str(&format!(
                "- `{}` / `{}`: {:.4} -> {:.4} ({:+.4})\n",
                improvement.strategy,
                improvement.metric,
                improvement.baseline,
                improvement.candidate,
                improvement.delta,
            ));
        }
    }
}

fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("evaluation report path has no valid file name")?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp"));
    fs::write(&temporary, content).with_context(|| {
        format!(
            "failed to write temporary evaluation report: {}",
            temporary.display()
        )
    })?;
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("failed to replace evaluation report: {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| {
        format!(
            "failed to publish evaluation report {} -> {}",
            temporary.display(),
            path.display()
        )
    })
}

fn format_rate(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |value| format!("{:.3}", value))
}

fn escape_table(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}
