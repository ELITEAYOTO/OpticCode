//! Reproducible, read-only evaluation for context and retrieval strategies.

mod metrics;
mod report;
mod runner;
mod schema;

pub use metrics::{calculate_retrieval_metrics, compare_reports, percentile, summarize_results};
pub use report::{load_eval_report, render_markdown_report, write_eval_reports, EvalReportPaths};
pub use runner::{load_eval_suite, run_evaluation, validate_eval_suite, EvalRunOptions};
pub use schema::*;

#[cfg(test)]
mod tests;
