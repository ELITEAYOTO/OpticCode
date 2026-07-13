//! Read-only Java context selection driven by task terms and cross-file symbols.

mod query;
mod ranking;
mod schema;
mod snippets;

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::build_project_context;
use crate::java_index::{analyze_java_index, JavaIndexOptions};

use query::parse_query;
use ranking::{rank_symbols, RankingLimits};
use schema::{estimate_tokens, JavaContextBudget as Budget};
pub use schema::{
    JavaContextBaselineComparison, JavaContextBudget, JavaContextCandidate, JavaContextCounts,
    JavaContextIgnored, JavaContextLimits, JavaContextMatchKind, JavaContextQuery,
    JavaContextScoreReason, JavaContextSnippet, JavaContextSnippetRole, JavaContextTimings,
    JavaContextTruncation, JavaTaskContextReport,
};
use snippets::{build_snippets, ProjectFileSelection};

pub const JAVA_CONTEXT_SCHEMA_VERSION: u32 = 1;
pub const MAX_JAVA_CONTEXT_TASK_CHARS: usize = 16 * 1024;
pub const DEFAULT_JAVA_CONTEXT_IDENTIFIER_LIMIT: usize = 64;
pub const MAX_JAVA_CONTEXT_IDENTIFIER_LIMIT: usize = 512;
pub const DEFAULT_JAVA_CONTEXT_TERM_LIMIT: usize = 96;
pub const MAX_JAVA_CONTEXT_TERM_LIMIT: usize = 1_024;
pub const DEFAULT_JAVA_CONTEXT_SYMBOL_VISIT_LIMIT: usize = 100_000;
pub const MAX_JAVA_CONTEXT_SYMBOL_VISIT_LIMIT: usize = 1_000_000;
pub const DEFAULT_JAVA_CONTEXT_PRIMARY_SYMBOL_LIMIT: usize = 8;
pub const MAX_JAVA_CONTEXT_PRIMARY_SYMBOL_LIMIT: usize = 64;
pub const DEFAULT_JAVA_CONTEXT_CANDIDATE_LIMIT: usize = 64;
pub const MAX_JAVA_CONTEXT_CANDIDATE_LIMIT: usize = 2_048;
pub const DEFAULT_JAVA_CONTEXT_RELATION_LIMIT: usize = 256;
pub const MAX_JAVA_CONTEXT_RELATION_LIMIT: usize = 8_192;
pub const DEFAULT_JAVA_CONTEXT_RELATION_DEPTH: usize = 1;
pub const MAX_JAVA_CONTEXT_RELATION_DEPTH: usize = 1;
pub const DEFAULT_JAVA_CONTEXT_SNIPPET_LIMIT: usize = 12;
pub const MAX_JAVA_CONTEXT_SNIPPET_LIMIT: usize = 128;
pub const DEFAULT_JAVA_CONTEXT_SNIPPET_BYTES: usize = 6 * 1024;
pub const MAX_JAVA_CONTEXT_SNIPPET_BYTES: usize = 128 * 1024;
pub const DEFAULT_JAVA_CONTEXT_BYTES: usize = 24 * 1024;
pub const MIN_JAVA_CONTEXT_BYTES: usize = 1_024;
pub const MAX_JAVA_CONTEXT_BYTES: usize = 1024 * 1024;
pub const DEFAULT_JAVA_CONTEXT_CHARS: usize = 24 * 1024;
pub const MIN_JAVA_CONTEXT_CHARS: usize = 256;
pub const MAX_JAVA_CONTEXT_CHARS: usize = 1024 * 1024;
pub const DEFAULT_JAVA_CONTEXT_TOKEN_LIMIT: usize = 6 * 1024;
pub const MIN_JAVA_CONTEXT_TOKEN_LIMIT: usize = 64;
pub const MAX_JAVA_CONTEXT_TOKEN_LIMIT: usize = 256 * 1024;
pub const DEFAULT_JAVA_CONTEXT_WARNING_LIMIT: usize = 128;
pub const MAX_JAVA_CONTEXT_WARNING_LIMIT: usize = 1_024;
pub const JAVA_CONTEXT_MIN_CANDIDATE_SCORE: u32 = 300;
pub const JAVA_CONTEXT_CANDIDATE_REASON_LIMIT: usize = 16;
pub const JAVA_CONTEXT_SNIPPET_REASON_LIMIT: usize = 4;
pub const DEFAULT_JAVA_CONTEXT_CALLER_LIMIT: usize = 4;
pub const MAX_JAVA_CONTEXT_CALLER_LIMIT: usize = 64;
pub const DEFAULT_JAVA_CONTEXT_RELATED_LIMIT: usize = 8;
pub const MAX_JAVA_CONTEXT_RELATED_LIMIT: usize = 128;

#[derive(Debug, Clone, Copy)]
pub struct JavaTaskContextOptions {
    pub index: JavaIndexOptions,
    pub max_query_identifiers: usize,
    pub max_query_terms: usize,
    pub max_symbols_visited: usize,
    pub max_primary_symbols: usize,
    pub max_candidates: usize,
    pub max_relations: usize,
    pub max_relation_depth: usize,
    pub max_snippets: usize,
    pub max_snippet_bytes: usize,
    pub max_context_bytes: usize,
    pub max_context_chars: usize,
    pub max_estimated_tokens: usize,
    pub max_warnings: usize,
    pub max_callers_per_symbol: usize,
    pub max_related_symbols: usize,
    pub compare_baseline: bool,
}

impl Default for JavaTaskContextOptions {
    fn default() -> Self {
        Self {
            index: JavaIndexOptions::default(),
            max_query_identifiers: DEFAULT_JAVA_CONTEXT_IDENTIFIER_LIMIT,
            max_query_terms: DEFAULT_JAVA_CONTEXT_TERM_LIMIT,
            max_symbols_visited: DEFAULT_JAVA_CONTEXT_SYMBOL_VISIT_LIMIT,
            max_primary_symbols: DEFAULT_JAVA_CONTEXT_PRIMARY_SYMBOL_LIMIT,
            max_candidates: DEFAULT_JAVA_CONTEXT_CANDIDATE_LIMIT,
            max_relations: DEFAULT_JAVA_CONTEXT_RELATION_LIMIT,
            max_relation_depth: DEFAULT_JAVA_CONTEXT_RELATION_DEPTH,
            max_snippets: DEFAULT_JAVA_CONTEXT_SNIPPET_LIMIT,
            max_snippet_bytes: DEFAULT_JAVA_CONTEXT_SNIPPET_BYTES,
            max_context_bytes: DEFAULT_JAVA_CONTEXT_BYTES,
            max_context_chars: DEFAULT_JAVA_CONTEXT_CHARS,
            max_estimated_tokens: DEFAULT_JAVA_CONTEXT_TOKEN_LIMIT,
            max_warnings: DEFAULT_JAVA_CONTEXT_WARNING_LIMIT,
            max_callers_per_symbol: DEFAULT_JAVA_CONTEXT_CALLER_LIMIT,
            max_related_symbols: DEFAULT_JAVA_CONTEXT_RELATED_LIMIT,
            compare_baseline: false,
        }
    }
}

pub fn build_java_task_context(
    input: &Path,
    task: &str,
    options: JavaTaskContextOptions,
) -> Result<JavaTaskContextReport> {
    validate_options(task, options)?;
    let started_at = Instant::now();

    let index_started = Instant::now();
    let index = analyze_java_index(input, options.index)?;
    let index_us = duration_us(index_started.elapsed());

    let query_started = Instant::now();
    let query = parse_query(task, options.max_query_identifiers, options.max_query_terms);
    let query_us = duration_us(query_started.elapsed());

    let ranking_started = Instant::now();
    let ranking = rank_symbols(
        &index,
        &query,
        RankingLimits {
            max_symbols_visited: options.max_symbols_visited,
            max_primary_symbols: options.max_primary_symbols,
            max_candidates: options.max_candidates,
            max_relations: options.max_relations,
            max_relation_depth: options.max_relation_depth,
            max_callers_per_symbol: options.max_callers_per_symbol,
            max_related_symbols: options.max_related_symbols,
        },
    );
    let ranking_us = duration_us(ranking_started.elapsed());

    let snippets_started = Instant::now();
    let built_snippets = build_snippets(
        &index,
        &ranking.candidates,
        &ranking.primary_symbols,
        options.max_snippets,
        options.max_snippet_bytes,
        ProjectFileSelection {
            build_manifest: query.requests_build_manifest(),
            bukkit_descriptor: query.requests_bukkit_descriptor(),
        },
    );
    let snippets_us = duration_us(snippets_started.elapsed());

    let diagnostics_observed = index.files.iter().fold(0usize, |total, file| {
        total.saturating_add(file.diagnostic_count)
    });
    let diagnostics_truncated = index
        .files
        .iter()
        .any(|file| file.diagnostic_count > index.limits.max_items_per_file_kind);
    let analysis_complete = index.analysis_complete
        && !index.truncation.candidates
        && !diagnostics_truncated
        && !query.report.truncated
        && !ranking.symbols_truncated
        && !ranking.relations_truncated
        && !ranking.relation_depth_truncated
        && built_snippets.source_read_errors == 0
        && built_snippets.source_hash_mismatches == 0;
    let mut truncation = JavaContextTruncation {
        index: index.truncated,
        diagnostics: diagnostics_truncated,
        warnings: false,
        query: query.report.truncated,
        symbols: ranking.symbols_truncated,
        candidates: ranking.candidates_truncated,
        relations: ranking.relations_truncated,
        relation_depth: ranking.relation_depth_truncated,
        snippets: built_snippets.snippets_truncated,
        context_bytes: false,
        context_chars: false,
        estimated_tokens: false,
    };
    let mut warnings = index.warnings.clone();
    warnings.extend(built_snippets.warnings);
    if ranking.candidates.is_empty() {
        warnings.push(
            "no Java symbol matched the task; only project metadata may be present".to_string(),
        );
    }
    if !index.analysis_complete {
        warnings.push(
            "symbol-guided context is incomplete because the Java index is incomplete".to_string(),
        );
    }
    if ranking.relation_depth_truncated {
        warnings.push(
            "additional symbol relations exist beyond the one-hop CONTEXT-001 scope".to_string(),
        );
    }

    let ignored = JavaContextIgnored {
        query_identifiers: query.omitted_identifiers,
        query_terms: query.omitted_terms,
        symbols: ranking.ignored_symbols,
        candidates: ranking
            .eligible_candidates
            .saturating_sub(ranking.candidates.len()),
        weak_candidates: ranking
            .scored_candidates
            .saturating_sub(ranking.eligible_candidates),
        relations: ranking.ignored_relations,
        snippets: built_snippets.omitted_snippets,
        warnings: 0,
    };

    let mut report = JavaTaskContextReport {
        schema_version: JAVA_CONTEXT_SCHEMA_VERSION,
        operation: "java_task_context",
        root: index.root.clone(),
        input: index.input.clone(),
        task: task.to_string(),
        limits: JavaContextLimits {
            index: index.limits,
            max_diagnostics_per_file: index.limits.max_items_per_file_kind,
            max_warnings: options.max_warnings,
            min_candidate_score: JAVA_CONTEXT_MIN_CANDIDATE_SCORE,
            max_score_reasons_per_candidate: JAVA_CONTEXT_CANDIDATE_REASON_LIMIT,
            max_selection_reasons_per_snippet: JAVA_CONTEXT_SNIPPET_REASON_LIMIT,
            max_query_identifiers: options.max_query_identifiers,
            max_query_terms: options.max_query_terms,
            max_symbols_visited: options.max_symbols_visited,
            max_primary_symbols: options.max_primary_symbols,
            max_candidates: options.max_candidates,
            max_relations: options.max_relations,
            max_relation_depth: options.max_relation_depth,
            max_snippets: options.max_snippets,
            max_snippet_bytes: options.max_snippet_bytes,
            max_context_bytes: options.max_context_bytes,
            max_context_chars: options.max_context_chars,
            max_estimated_tokens: options.max_estimated_tokens,
            max_callers_per_symbol: options.max_callers_per_symbol,
            max_related_symbols: options.max_related_symbols,
        },
        query: query.report,
        index_schema_version: index.schema_version,
        index_source: index.source,
        index_counts: index.counts,
        index_truncation: index.truncation,
        index_analysis_complete: index.analysis_complete,
        analysis_complete,
        selection_complete: false,
        truncated: false,
        truncation: JavaContextTruncation::default(),
        primary_symbol: ranking.primary_symbol,
        primary_symbols: ranking.primary_symbols,
        primary_ambiguous: ranking.primary_ambiguous,
        counts: JavaContextCounts {
            indexed_symbols: index.symbols.len(),
            indexed_references: index.references.len(),
            diagnostics_observed,
            visited_symbols: ranking.visited_symbols,
            relations_examined: ranking.relations_examined,
            relations_followed: ranking.relations_followed,
            deepest_relation_depth: ranking.deepest_relation_depth,
            relation_cycles_skipped: ranking.relation_cycles_skipped,
            invalid_context_symbols_ignored: ranking.invalid_context_symbols_ignored,
            invalid_context_references_ignored: ranking.invalid_context_references_ignored,
            scored_candidates: ranking.scored_candidates,
            eligible_candidates: ranking.eligible_candidates,
            retained_candidates: ranking.candidates.len(),
            primary_score_ties: ranking.primary_score_ties,
            primary_match_ties: ranking.primary_match_ties,
            source_reads: built_snippets.source_reads,
            source_read_errors: built_snippets.source_read_errors,
            source_hash_mismatches: built_snippets.source_hash_mismatches,
            ..JavaContextCounts::default()
        },
        ignored,
        budget: Budget {
            max_context_bytes: options.max_context_bytes,
            max_context_chars: options.max_context_chars,
            max_estimated_tokens: options.max_estimated_tokens,
            token_estimator: "ceil_unicode_chars_div_4",
            ..Budget::default()
        },
        timings: JavaContextTimings {
            index_us,
            query_us,
            ranking_us,
            snippets_us,
            baseline_us: 0,
            total_us: 0,
            serialization_us: None,
        },
        candidates: ranking.candidates,
        snippets: built_snippets.snippets,
        baseline_comparison: None,
        warnings,
    };
    report.refresh_budget_metrics();
    loop {
        let bytes_exceeded = report.budget.rendered_bytes > options.max_context_bytes;
        let chars_exceeded = report.budget.rendered_chars > options.max_context_chars;
        let tokens_exceeded = report.budget.estimated_tokens > options.max_estimated_tokens;
        if !bytes_exceeded && !chars_exceeded && !tokens_exceeded {
            break;
        }
        truncation.context_bytes |= bytes_exceeded;
        truncation.context_chars |= chars_exceeded;
        truncation.estimated_tokens |= tokens_exceeded;
        let Some(_) = report.snippets.pop() else {
            bail!(
                "Java context metadata exceeds configured budgets ({} bytes, {} chars, ~{} tokens)",
                report.budget.rendered_bytes,
                report.budget.rendered_chars,
                report.budget.estimated_tokens
            );
        };
        report.ignored.snippets = report.ignored.snippets.saturating_add(1);
        truncation.snippets = true;
        report.refresh_budget_metrics();
    }

    if options.compare_baseline {
        let baseline_started = Instant::now();
        match build_project_context(&report.root) {
            Ok(baseline) => {
                let baseline_prompt = baseline.to_prompt_context();
                let baseline_tokens = estimate_tokens(&baseline_prompt);
                let selected_tokens = report.budget.estimated_tokens;
                let delta = baseline_tokens as i64 - selected_tokens as i64;
                let reduction_basis_points = if baseline_tokens == 0 {
                    0
                } else {
                    ((delta.saturating_mul(10_000)) / baseline_tokens as i64)
                        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
                };
                report.baseline_comparison = Some(JavaContextBaselineComparison {
                    baseline: "legacy_file_priority_v1",
                    baseline_files: baseline.snippets.len(),
                    baseline_rendered_bytes: baseline_prompt.len(),
                    baseline_rendered_chars: baseline_prompt.chars().count(),
                    baseline_estimated_tokens: baseline_tokens,
                    selected_files: report.counts.selected_files,
                    selected_rendered_bytes: report.budget.rendered_bytes,
                    selected_rendered_chars: report.budget.rendered_chars,
                    selected_estimated_tokens: selected_tokens,
                    estimated_token_delta: delta,
                    estimated_token_reduction_basis_points: reduction_basis_points,
                });
            }
            Err(error) => report.warnings.push(format!(
                "legacy context baseline comparison failed: {error:#}"
            )),
        }
        report.timings.baseline_us = duration_us(baseline_started.elapsed());
    }

    if report.warnings.len() > options.max_warnings {
        report.ignored.warnings = report.warnings.len().saturating_sub(options.max_warnings);
        report.warnings.truncate(options.max_warnings);
        truncation.warnings = true;
    }
    report.counts.warnings = report.warnings.len();
    report.truncation = truncation;
    report.truncated = report.truncation.any();
    report.selection_complete = report.analysis_complete && !report.truncated;
    report.timings.total_us = duration_us(started_at.elapsed());
    Ok(report)
}

fn validate_options(task: &str, options: JavaTaskContextOptions) -> Result<()> {
    if task.trim().is_empty() {
        bail!("Java context task must not be empty");
    }
    if task.chars().count() > MAX_JAVA_CONTEXT_TASK_CHARS {
        bail!(
            "Java context task exceeds the {} character limit",
            MAX_JAVA_CONTEXT_TASK_CHARS
        );
    }
    validate_limit(
        "query identifier",
        options.max_query_identifiers,
        MAX_JAVA_CONTEXT_IDENTIFIER_LIMIT,
    )?;
    validate_limit(
        "query term",
        options.max_query_terms,
        MAX_JAVA_CONTEXT_TERM_LIMIT,
    )?;
    validate_limit(
        "visited symbol",
        options.max_symbols_visited,
        MAX_JAVA_CONTEXT_SYMBOL_VISIT_LIMIT,
    )?;
    validate_limit(
        "primary symbol",
        options.max_primary_symbols,
        MAX_JAVA_CONTEXT_PRIMARY_SYMBOL_LIMIT,
    )?;
    validate_limit(
        "candidate",
        options.max_candidates,
        MAX_JAVA_CONTEXT_CANDIDATE_LIMIT,
    )?;
    if options.max_primary_symbols > options.max_candidates {
        bail!("Java context primary symbol limit must not exceed the candidate limit");
    }
    validate_limit(
        "relation",
        options.max_relations,
        MAX_JAVA_CONTEXT_RELATION_LIMIT,
    )?;
    validate_limit(
        "relation depth",
        options.max_relation_depth,
        MAX_JAVA_CONTEXT_RELATION_DEPTH,
    )?;
    validate_limit(
        "snippet",
        options.max_snippets,
        MAX_JAVA_CONTEXT_SNIPPET_LIMIT,
    )?;
    validate_limit(
        "snippet byte",
        options.max_snippet_bytes,
        MAX_JAVA_CONTEXT_SNIPPET_BYTES,
    )?;
    if options.max_context_bytes < MIN_JAVA_CONTEXT_BYTES
        || options.max_context_bytes > MAX_JAVA_CONTEXT_BYTES
    {
        bail!(
            "Java context byte limit must be between {} and {}",
            MIN_JAVA_CONTEXT_BYTES,
            MAX_JAVA_CONTEXT_BYTES
        );
    }
    if options.max_snippet_bytes > options.max_context_bytes {
        bail!("Java context snippet byte limit must not exceed the total context budget");
    }
    if options.max_context_chars < MIN_JAVA_CONTEXT_CHARS
        || options.max_context_chars > MAX_JAVA_CONTEXT_CHARS
    {
        bail!(
            "Java context character limit must be between {} and {}",
            MIN_JAVA_CONTEXT_CHARS,
            MAX_JAVA_CONTEXT_CHARS
        );
    }
    if options.max_estimated_tokens < MIN_JAVA_CONTEXT_TOKEN_LIMIT
        || options.max_estimated_tokens > MAX_JAVA_CONTEXT_TOKEN_LIMIT
    {
        bail!(
            "Java context token limit must be between {} and {}",
            MIN_JAVA_CONTEXT_TOKEN_LIMIT,
            MAX_JAVA_CONTEXT_TOKEN_LIMIT
        );
    }
    validate_limit(
        "warning",
        options.max_warnings,
        MAX_JAVA_CONTEXT_WARNING_LIMIT,
    )?;
    validate_limit(
        "caller",
        options.max_callers_per_symbol,
        MAX_JAVA_CONTEXT_CALLER_LIMIT,
    )?;
    validate_limit(
        "related symbol",
        options.max_related_symbols,
        MAX_JAVA_CONTEXT_RELATED_LIMIT,
    )?;
    Ok(())
}

fn validate_limit(label: &str, value: usize, maximum: usize) -> Result<()> {
    if value == 0 || value > maximum {
        bail!("Java context {label} limit must be between 1 and {maximum}");
    }
    Ok(())
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{build_java_task_context, JavaTaskContextOptions};
    use crate::java_context::JavaContextSnippetRole;
    use crate::java_index::JavaIndexOptions;
    use crate::java_syntax::JavaSyntaxOptions;

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn selects_exact_method_callers_and_related_symbols_without_writes() {
        let root = corpus_root();
        let before = snapshot_tree(&root);
        let report = build_java_task_context(
            &root,
            "Modifier dev.opticcode.util.Helpers#create(String) et verifier ses appelants",
            JavaTaskContextOptions {
                compare_baseline: true,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("symbol context should build");
        let after = snapshot_tree(&root);

        assert_eq!(
            before, after,
            "context selection modified its source corpus"
        );
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.operation, "java_task_context");
        assert!(report.index_analysis_complete);
        assert!(!report.analysis_complete);
        assert!(report.truncation.relation_depth);
        assert_eq!(
            report.primary_symbol.as_deref(),
            Some("dev.opticcode.util.Helpers#create(String)")
        );
        assert!(!report.primary_ambiguous);
        assert!(report.snippets.iter().any(|snippet| {
            snippet.role == JavaContextSnippetRole::PrimaryDeclaration
                && snippet
                    .content
                    .contains("static String create(String value)")
        }));
        assert!(report.snippets.iter().any(|snippet| {
            snippet.role == JavaContextSnippetRole::Caller
                && snippet.content.contains("void start()")
        }));
        assert_eq!(report.counts.source_reads, report.counts.selected_files);
        assert_eq!(
            report.limits.min_candidate_score,
            super::JAVA_CONTEXT_MIN_CANDIDATE_SCORE
        );
        assert_eq!(
            report.counts.scored_candidates,
            report.counts.eligible_candidates + report.ignored.weak_candidates
        );
        assert_eq!(
            report.counts.eligible_candidates,
            report.counts.retained_candidates + report.ignored.candidates
        );
        assert!(report.snippets.iter().all(|snippet| {
            snippet.content_chars == snippet.content.chars().count()
                && snippet.estimated_tokens == super::estimate_tokens(&snippet.content)
                && snippet.selection_reasons.len()
                    <= report.limits.max_selection_reasons_per_snippet
        }));
        assert!(report.candidates.iter().all(|candidate| {
            candidate.reasons.len() <= report.limits.max_score_reasons_per_candidate
                && candidate.reason_count >= candidate.reasons.len()
                && candidate.reasons_truncated == (candidate.reason_count > candidate.reasons.len())
        }));
        assert!(report.budget.rendered_bytes <= report.limits.max_context_bytes);
        assert!(report.baseline_comparison.is_some());
    }

    #[test]
    fn enforces_rendered_budget_and_reports_truncation() {
        let report = build_java_task_context(
            &corpus_root(),
            "Plugin start create material service",
            JavaTaskContextOptions {
                max_snippet_bytes: 512,
                max_context_bytes: 1_024,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("bounded context should build");

        assert!(report.budget.rendered_bytes <= 1_024);
        assert!(
            report.truncation.context_bytes
                || report.truncation.context_chars
                || report.truncation.estimated_tokens
                || report.truncation.snippets
        );
        assert!(report.truncated);
        assert!(!report.snippets.is_empty());
    }

    #[test]
    fn distinguishes_overloads_and_reports_ambiguous_simple_names() {
        let no_argument = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.util.Helpers#create()",
            JavaTaskContextOptions::default(),
        )
        .expect("no-argument overload context");
        let string_argument = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.util.Helpers#create(String)",
            JavaTaskContextOptions::default(),
        )
        .expect("String overload context");

        assert_eq!(
            no_argument.primary_symbol.as_deref(),
            Some("dev.opticcode.util.Helpers#create()")
        );
        assert_eq!(
            string_argument.primary_symbol.as_deref(),
            Some("dev.opticcode.util.Helpers#create(String)")
        );
        assert_ne!(no_argument.primary_symbol, string_argument.primary_symbol);

        let ambiguous = build_java_task_context(
            &corpus_root(),
            "Duplicate",
            JavaTaskContextOptions::default(),
        )
        .expect("ambiguous context");
        let primary = ambiguous
            .primary_symbols
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert!(ambiguous.primary_ambiguous);
        assert!(ambiguous.counts.primary_match_ties >= 2);
        assert!(primary.contains("dev.opticcode.alpha.Duplicate"));
        assert!(primary.contains("dev.opticcode.beta.Duplicate"));
        let ambiguous_prompt = ambiguous.to_prompt_context();
        assert!(ambiguous_prompt.contains("dev.opticcode.alpha.Duplicate"));
        assert!(ambiguous_prompt.contains("dev.opticcode.beta.Duplicate"));

        let multiple_explicit = build_java_task_context(
            &corpus_root(),
            "Helpers et Plugin",
            JavaTaskContextOptions::default(),
        )
        .expect("multiple explicit symbols");
        assert!(!multiple_explicit.primary_ambiguous);
        assert!(multiple_explicit
            .primary_symbols
            .iter()
            .any(|symbol| symbol == "dev.opticcode.util.Helpers"));
        assert!(multiple_explicit
            .primary_symbols
            .iter()
            .any(|symbol| symbol == "dev.opticcode.app.Plugin"));
    }

    #[test]
    fn unresolved_symbols_do_not_invent_declarations() {
        let report = build_java_task_context(
            &corpus_root(),
            "MissingType",
            JavaTaskContextOptions::default(),
        )
        .expect("unresolved reference context");

        assert!(report
            .candidates
            .iter()
            .all(|candidate| !candidate.symbol_id.contains("MissingType")));
        assert!(report
            .snippets
            .iter()
            .all(|snippet| snippet.symbol_id.as_deref() != Some("MissingType")));
        assert_eq!(
            report.primary_symbol.as_deref(),
            Some("dev.opticcode.app.Plugin")
        );
        assert!(report.snippets.iter().any(|snippet| {
            snippet.role == JavaContextSnippetRole::PrimaryDeclaration
                && snippet.symbol_id.as_deref() == Some("dev.opticcode.app.Plugin")
        }));
        assert!(report.snippets.iter().all(|snippet| {
            snippet.role != JavaContextSnippetRole::PrimaryDeclaration
                || snippet
                    .symbol_id
                    .as_ref()
                    .is_some_and(|symbol| report.primary_symbols.contains(symbol))
        }));
    }

    #[test]
    fn includes_only_configuration_relevant_to_the_task() {
        let descriptor = build_java_task_context(
            &corpus_root(),
            "Ajouter une commande Bukkit et sa permission dans plugin.yml",
            JavaTaskContextOptions::default(),
        )
        .expect("descriptor context");
        assert!(descriptor.snippets.iter().any(|snippet| {
            snippet.role == JavaContextSnippetRole::BukkitDescriptor
                && snippet.file == Path::new("src/main/resources/plugin.yml")
        }));
        assert!(!descriptor
            .snippets
            .iter()
            .any(|snippet| snippet.role == JavaContextSnippetRole::BuildManifest));
        let descriptor_under_pressure = build_java_task_context(
            &corpus_root(),
            "Plugin command permission plugin.yml",
            JavaTaskContextOptions {
                max_snippets: 1,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("descriptor context under snippet pressure");
        assert_eq!(descriptor_under_pressure.snippets.len(), 1);
        assert_eq!(
            descriptor_under_pressure.snippets[0].role,
            JavaContextSnippetRole::BukkitDescriptor
        );

        let manifest = build_java_task_context(
            &corpus_root(),
            "Mettre a jour la dependency Maven dans pom.xml",
            JavaTaskContextOptions::default(),
        )
        .expect("manifest context");
        assert!(manifest.snippets.iter().any(|snippet| {
            snippet.role == JavaContextSnippetRole::BuildManifest
                && snippet.file == Path::new("pom.xml")
        }));
        assert!(!manifest
            .snippets
            .iter()
            .any(|snippet| snippet.role == JavaContextSnippetRole::BukkitDescriptor));

        let source_only = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.util.Helpers#create(String)",
            JavaTaskContextOptions::default(),
        )
        .expect("source-only context");
        assert!(source_only.snippets.iter().all(|snippet| {
            !matches!(
                snippet.role,
                JavaContextSnippetRole::BuildManifest | JavaContextSnippetRole::BukkitDescriptor
            )
        }));
        assert!(source_only
            .snippets
            .iter()
            .all(|snippet| !snippet.file.to_string_lossy().contains("Duplicate.java")));
        let plugin_source_only = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.app.Plugin#start()",
            JavaTaskContextOptions::default(),
        )
        .expect("plugin source-only context");
        assert!(plugin_source_only
            .snippets
            .iter()
            .all(|snippet| snippet.role != JavaContextSnippetRole::BukkitDescriptor));
    }

    #[test]
    fn enforces_snippet_character_token_and_symbol_budgets_independently() {
        let snippets = build_java_task_context(
            &corpus_root(),
            "Plugin start create material service",
            JavaTaskContextOptions {
                max_snippets: 1,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("snippet-limited context");
        assert_eq!(snippets.snippets.len(), 1);
        assert!(snippets.truncation.snippets);
        assert!(snippets.ignored.snippets > 0);

        let characters = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.util.Helpers#create(String)",
            JavaTaskContextOptions {
                max_context_chars: 256,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("character-limited context");
        assert!(characters.budget.rendered_chars <= 256);
        assert!(characters.truncation.context_chars);

        let tokens = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.util.Helpers#create(String)",
            JavaTaskContextOptions {
                max_estimated_tokens: 64,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("token-limited context");
        assert!(tokens.budget.estimated_tokens <= 64);
        assert!(tokens.truncation.estimated_tokens);

        let symbols = build_java_task_context(
            &corpus_root(),
            "Plugin",
            JavaTaskContextOptions {
                max_symbols_visited: 1,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("symbol-limited context");
        assert_eq!(symbols.counts.visited_symbols, 1);
        assert!(symbols.truncation.symbols);
        assert!(symbols.ignored.symbols > 0);
        assert!(!symbols.analysis_complete);
    }

    #[test]
    fn bounds_relation_depth_and_stops_cycles_with_a_visited_set() {
        let chain = TemporaryProject::new("context-chain");
        chain.write(
            "src/main/java/test/A.java",
            b"package test; public final class A { public static void start() { B.middle(); } }\n",
        );
        chain.write(
            "src/main/java/test/B.java",
            b"package test; public final class B { public static void middle() { C.end(); } }\n",
        );
        chain.write(
            "src/main/java/test/C.java",
            b"package test; public final class C { public static void end() { } }\n",
        );
        chain.write(
            "src/main/java/test/D.java",
            b"package test; public final class D { public static void call() { A.start(); } }\n",
        );
        let chain_report = build_java_task_context(
            &chain.root,
            "test.A#start()",
            JavaTaskContextOptions::default(),
        )
        .expect("bounded chain context");
        assert!(chain_report.counts.deepest_relation_depth <= 1);
        assert!(chain_report.truncation.relation_depth);
        assert!(chain_report.ignored.relations > 0);
        assert!(chain_report
            .candidates
            .iter()
            .any(|candidate| candidate.symbol_id == "test.B#middle()"));
        assert!(chain_report
            .candidates
            .iter()
            .all(|candidate| candidate.symbol_id != "test.C#end()"));

        let relation_limited = build_java_task_context(
            &chain.root,
            "test.A#start()",
            JavaTaskContextOptions {
                max_relations: 1,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("relation-limited context");
        assert_eq!(relation_limited.counts.relations_followed, 1);
        assert!(relation_limited.truncation.relations);
        assert!(relation_limited.ignored.relations > 0);

        let cycle = TemporaryProject::new("context-cycle");
        cycle.write(
            "src/main/java/test/A.java",
            b"package test; public final class A { public static void start() { B.middle(); } }\n",
        );
        cycle.write(
            "src/main/java/test/B.java",
            b"package test; public final class B { public static void middle() { A.start(); } }\n",
        );
        let cycle_report = build_java_task_context(
            &cycle.root,
            "test.A#start()",
            JavaTaskContextOptions::default(),
        )
        .expect("cycle context");
        let candidate_ids = cycle_report
            .candidates
            .iter()
            .map(|candidate| candidate.symbol_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(candidate_ids.len(), cycle_report.candidates.len());
        assert!(cycle_report.counts.relation_cycles_skipped > 0);
        assert!(cycle_report.counts.deepest_relation_depth <= 1);
    }

    #[test]
    fn handles_unicode_crlf_and_fails_closed_on_invalid_java() {
        let unicode = TemporaryProject::new("context-unicode");
        unicode.write(
            "src/main/java/unicode/UnicodeService.java",
            concat!(
                "package unicode;\r\n",
                "public final class UnicodeService {\r\n",
                "    public String saluer() {\r\n",
                "        return \"Bonjour, étoile brillante\";\r\n",
                "    }\r\n",
                "}\r\n",
            )
            .as_bytes(),
        );
        let unicode_report = build_java_task_context(
            &unicode.root,
            "unicode.UnicodeService#saluer()",
            JavaTaskContextOptions::default(),
        )
        .expect("Unicode CRLF context");
        let snippet = unicode_report
            .snippets
            .first()
            .expect("Unicode declaration snippet");
        assert!(snippet.content.contains("étoile brillante"));
        assert!(snippet.content.contains("\r\n"));
        assert_eq!(snippet.content_chars, snippet.content.chars().count());

        let invalid = TemporaryProject::new("context-invalid");
        invalid.write(
            "src/main/java/broken/Broken.java",
            concat!(
                "package broken; public class Broken {\n",
                "  void run( { Missing.call( }\n",
                "  void stop( { Other.call( }\n",
                "}\n",
            )
            .as_bytes(),
        );
        let invalid_report = build_java_task_context(
            &invalid.root,
            "broken.Broken#run()",
            JavaTaskContextOptions::default(),
        )
        .expect("invalid Java should return a bounded report");
        assert!(!invalid_report.analysis_complete);
        assert!(!invalid_report.index_analysis_complete);
        assert!(invalid_report.candidates.is_empty());
        assert!(invalid_report.snippets.is_empty());
        assert!(invalid_report.counts.diagnostics_observed > 0);

        let diagnostics_limited = build_java_task_context(
            &invalid.root,
            "broken.Broken#run()",
            JavaTaskContextOptions {
                index: JavaIndexOptions {
                    syntax: JavaSyntaxOptions {
                        max_items_per_kind: 1,
                        ..JavaSyntaxOptions::default()
                    },
                    ..JavaIndexOptions::default()
                },
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("diagnostic-limited report");
        assert!(diagnostics_limited.counts.diagnostics_observed > 1);
        assert!(diagnostics_limited.truncation.diagnostics);
    }

    #[test]
    fn caps_warnings_and_serializes_deterministically() {
        let warnings = TemporaryProject::new("context-warnings");
        for index in 0..5 {
            warnings.write(
                format!("src/main/java/bad/Bad{index}.java"),
                &[0xff, 0xfe, index as u8],
            );
        }
        let warning_report = build_java_task_context(
            &warnings.root,
            "Bad",
            JavaTaskContextOptions {
                max_warnings: 2,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect("warning-bounded context");
        assert_eq!(warning_report.warnings.len(), 2);
        assert!(warning_report.truncation.warnings);
        assert!(warning_report.ignored.warnings > 0);

        let first = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.util.Helpers#create(String)",
            JavaTaskContextOptions::default(),
        )
        .expect("first deterministic report");
        let second = build_java_task_context(
            &corpus_root(),
            "dev.opticcode.util.Helpers#create(String)",
            JavaTaskContextOptions::default(),
        )
        .expect("second deterministic report");
        let mut first_json = serde_json::to_value(first).expect("serialize first report");
        let mut second_json = serde_json::to_value(second).expect("serialize second report");
        first_json
            .as_object_mut()
            .expect("report object")
            .remove("timings");
        second_json
            .as_object_mut()
            .expect("report object")
            .remove("timings");
        assert_eq!(first_json, second_json);
    }

    #[test]
    fn rejects_empty_tasks_and_invalid_limits() {
        let error = build_java_task_context(&corpus_root(), " ", JavaTaskContextOptions::default())
            .expect_err("empty task should fail");
        assert!(error.to_string().contains("must not be empty"));

        let error = build_java_task_context(
            &corpus_root(),
            "create",
            JavaTaskContextOptions {
                max_candidates: 0,
                ..JavaTaskContextOptions::default()
            },
        )
        .expect_err("zero candidates should fail");
        assert!(error.to_string().contains("candidate limit"));
    }

    fn corpus_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-index-mini")
    }

    struct TemporaryProject {
        root: PathBuf,
    }

    impl TemporaryProject {
        fn new(label: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after Unix epoch")
                .as_nanos();
            let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "opticcode-{label}-{}-{stamp}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create temporary Java project");
            Self { root }
        }

        fn write(&self, relative: impl AsRef<Path>, bytes: &[u8]) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create temporary source parent");
            }
            fs::write(path, bytes).expect("write temporary source");
        }
    }

    impl Drop for TemporaryProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn visit(root: &Path, current: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(current).expect("read corpus directory") {
                let entry = entry.expect("read corpus entry");
                let path = entry.path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root)
                            .expect("relative corpus path")
                            .to_string_lossy()
                            .replace('\\', "/"),
                        fs::read(path).expect("read corpus file"),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }
}
