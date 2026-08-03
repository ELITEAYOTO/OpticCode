use std::path::PathBuf;

use serde::Serialize;

use crate::java_index::{
    JavaIndexCounts, JavaIndexLimits, JavaIndexSourceSummary, JavaIndexTruncation,
    JavaIndexedSymbolKind,
};
use crate::java_syntax::SourceRange;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct JavaContextLimits {
    pub index: JavaIndexLimits,
    pub max_diagnostics_per_file: usize,
    pub max_warnings: usize,
    pub min_candidate_score: u32,
    pub max_score_reasons_per_candidate: usize,
    pub max_selection_reasons_per_snippet: usize,
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
    pub max_callers_per_symbol: usize,
    pub max_related_symbols: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaContextQuery {
    pub raw: String,
    pub normalized: String,
    pub raw_chars: usize,
    pub identifiers: Vec<String>,
    pub terms: Vec<String>,
    pub ignored_terms: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaContextCounts {
    pub indexed_symbols: usize,
    pub indexed_references: usize,
    pub diagnostics_observed: usize,
    pub visited_symbols: usize,
    pub relations_examined: usize,
    pub relations_followed: usize,
    pub deepest_relation_depth: usize,
    pub relation_cycles_skipped: usize,
    pub invalid_context_symbols_ignored: usize,
    pub invalid_context_references_ignored: usize,
    pub scored_candidates: usize,
    pub eligible_candidates: usize,
    pub retained_candidates: usize,
    pub selected_files: usize,
    pub snippets: usize,
    pub primary_score_ties: usize,
    pub primary_match_ties: usize,
    pub source_reads: usize,
    pub source_read_errors: usize,
    pub source_hash_mismatches: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaContextIgnored {
    pub query_identifiers: usize,
    pub query_terms: usize,
    pub symbols: usize,
    pub candidates: usize,
    pub weak_candidates: usize,
    pub relations: usize,
    pub snippets: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaContextTruncation {
    pub index: bool,
    pub diagnostics: bool,
    pub warnings: bool,
    pub query: bool,
    pub symbols: bool,
    pub candidates: bool,
    pub relations: bool,
    pub relation_depth: bool,
    pub snippets: bool,
    pub context_bytes: bool,
    pub context_chars: bool,
    pub estimated_tokens: bool,
}

impl JavaContextTruncation {
    pub fn any(&self) -> bool {
        self.index
            || self.diagnostics
            || self.warnings
            || self.query
            || self.symbols
            || self.candidates
            || self.relations
            || self.relation_depth
            || self.snippets
            || self.context_bytes
            || self.context_chars
            || self.estimated_tokens
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaContextTimings {
    pub index_us: u64,
    pub query_us: u64,
    pub ranking_us: u64,
    pub snippets_us: u64,
    pub baseline_us: u64,
    pub total_us: u64,
    pub serialization_us: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaContextBudget {
    pub max_context_bytes: usize,
    pub max_context_chars: usize,
    pub max_estimated_tokens: usize,
    pub content_bytes: usize,
    pub rendered_bytes: usize,
    pub rendered_chars: usize,
    pub estimated_tokens: usize,
    pub token_estimator: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaContextMatchKind {
    ExactSymbolId,
    ExactQualifiedName,
    ExactSignature,
    ExactName,
    IdentifierSuffix,
    SymbolTerm,
    QualifiedTerm,
    FileTerm,
    MatchingReferenceTarget,
    MatchingReferenceOwner,
    CallerOfPrimary,
    ReferencedByPrimary,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaContextScoreReason {
    pub kind: JavaContextMatchKind,
    pub score: u32,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaContextCandidate {
    pub symbol_id: String,
    pub kind: JavaIndexedSymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub signature: Option<String>,
    pub file: PathBuf,
    pub source_hash: String,
    pub range: SourceRange,
    pub name_range: SourceRange,
    pub score: u32,
    pub reason_count: usize,
    pub reasons_truncated: bool,
    pub reasons: Vec<JavaContextScoreReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaContextSnippetRole {
    PrimaryDeclaration,
    Caller,
    RelatedDeclaration,
    SupportingDeclaration,
    BuildManifest,
    BukkitDescriptor,
}

impl JavaContextSnippetRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryDeclaration => "primary_declaration",
            Self::Caller => "caller",
            Self::RelatedDeclaration => "related_declaration",
            Self::SupportingDeclaration => "supporting_declaration",
            Self::BuildManifest => "build_manifest",
            Self::BukkitDescriptor => "bukkit_descriptor",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaContextSnippet {
    pub id: String,
    pub role: JavaContextSnippetRole,
    pub file: PathBuf,
    pub symbol_id: Option<String>,
    pub source_hash: String,
    pub ast_range: Option<SourceRange>,
    pub content_range: Option<SourceRange>,
    pub score: u32,
    pub selection_reasons: Vec<String>,
    pub selection_reasons_truncated: bool,
    pub original_bytes: usize,
    pub content_bytes: usize,
    pub content_chars: usize,
    pub estimated_tokens: usize,
    pub content_hash: String,
    pub truncated: bool,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaContextBaselineComparison {
    pub baseline: &'static str,
    pub baseline_files: usize,
    pub baseline_rendered_bytes: usize,
    pub baseline_rendered_chars: usize,
    pub baseline_estimated_tokens: usize,
    pub selected_files: usize,
    pub selected_rendered_bytes: usize,
    pub selected_rendered_chars: usize,
    pub selected_estimated_tokens: usize,
    pub estimated_token_delta: i64,
    pub estimated_token_reduction_basis_points: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaTaskContextReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub root: PathBuf,
    pub input: PathBuf,
    pub task: String,
    pub limits: JavaContextLimits,
    pub query: JavaContextQuery,
    pub index_schema_version: u32,
    pub index_source: JavaIndexSourceSummary,
    pub index_counts: JavaIndexCounts,
    pub index_truncation: JavaIndexTruncation,
    pub index_analysis_complete: bool,
    pub analysis_complete: bool,
    pub selection_complete: bool,
    pub truncated: bool,
    pub truncation: JavaContextTruncation,
    pub primary_symbol: Option<String>,
    pub primary_symbols: Vec<String>,
    pub primary_ambiguous: bool,
    pub counts: JavaContextCounts,
    pub ignored: JavaContextIgnored,
    pub budget: JavaContextBudget,
    pub timings: JavaContextTimings,
    pub candidates: Vec<JavaContextCandidate>,
    pub snippets: Vec<JavaContextSnippet>,
    pub baseline_comparison: Option<JavaContextBaselineComparison>,
    pub warnings: Vec<String>,
}

impl JavaTaskContextReport {
    pub fn to_display_string(&self) -> String {
        let mut output = format!(
            concat!(
                "Java task context (read-only):\n",
                "- root: {}\n",
                "- primary symbol: {}\n",
                "- primary ambiguous: {}\n",
                "- candidates: {}/{} scored\n",
                "- snippets: {} in {} files\n",
                "- rendered context: {} bytes, ~{} tokens\n",
                "- analysis complete: {}\n",
                "- selection complete: {}\n",
                "- truncated: {}\n",
                "- duration: {:.3} ms\n"
            ),
            self.root.display(),
            self.primary_symbol.as_deref().unwrap_or("none"),
            self.primary_ambiguous,
            self.counts.retained_candidates,
            self.counts.scored_candidates,
            self.counts.snippets,
            self.counts.selected_files,
            self.budget.rendered_bytes,
            self.budget.estimated_tokens,
            self.analysis_complete,
            self.selection_complete,
            self.truncated,
            self.timings.total_us as f64 / 1_000.0,
        );

        for candidate in self.candidates.iter().take(10) {
            output.push_str(&format!(
                "- candidate {} (score {}, {})\n",
                candidate.symbol_id,
                candidate.score,
                candidate.file.display()
            ));
        }
        for snippet in &self.snippets {
            output.push_str(&format!(
                "- snippet {} [{}] {} bytes\n",
                snippet.file.display(),
                snippet.role.as_str(),
                snippet.content_bytes
            ));
        }
        if let Some(comparison) = &self.baseline_comparison {
            output.push_str(&format!(
                "- baseline comparison: ~{} -> ~{} tokens ({:+.2}%)\n",
                comparison.baseline_estimated_tokens,
                comparison.selected_estimated_tokens,
                comparison.estimated_token_reduction_basis_points as f64 / 100.0,
            ));
        }
        for warning in &self.warnings {
            output.push_str(&format!("Warning: {warning}\n"));
        }
        output
    }

    pub fn to_prompt_context(&self) -> String {
        render_prompt_context(
            &self.primary_symbols,
            self.primary_ambiguous,
            self.analysis_complete,
            &self.snippets,
        )
    }

    pub(crate) fn refresh_budget_metrics(&mut self) {
        let rendered = self.to_prompt_context();
        self.budget.content_bytes = self.snippets.iter().fold(0usize, |total, snippet| {
            total.saturating_add(snippet.content_bytes)
        });
        self.budget.rendered_bytes = rendered.len();
        self.budget.rendered_chars = rendered.chars().count();
        self.budget.estimated_tokens = estimate_tokens(&rendered);
        self.counts.snippets = self.snippets.len();
        self.counts.selected_files = self
            .snippets
            .iter()
            .map(|snippet| &snippet.file)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
    }
}

pub(crate) fn render_prompt_context(
    primary_symbols: &[String],
    primary_ambiguous: bool,
    analysis_complete: bool,
    snippets: &[JavaContextSnippet],
) -> String {
    let mut output = String::new();
    output.push_str("Java project context selected by symbols\n");
    output.push_str(&format!(
        "Primary symbols: {}\nPrimary ambiguous: {}\nAnalysis complete: {}\n",
        if primary_symbols.is_empty() {
            "none".to_string()
        } else {
            primary_symbols.join(", ")
        },
        primary_ambiguous,
        analysis_complete
    ));

    for snippet in snippets {
        output.push_str(&format!(
            "\n--- {} | {}",
            snippet.file.display(),
            snippet.role.as_str()
        ));
        if let Some(symbol_id) = &snippet.symbol_id {
            output.push_str(&format!(" | {symbol_id}"));
        }
        if let Some(range) = snippet.content_range {
            output.push_str(&format!(
                " | lines {}-{}",
                range.start.row + 1,
                range.end.row + 1
            ));
        }
        output.push_str(" ---\n");
        output.push_str(&snippet.content);
        if !snippet.content.ends_with('\n') {
            output.push('\n');
        }
        if snippet.truncated {
            output.push_str("[snippet truncated]\n");
        }
    }
    output
}

pub(crate) fn estimate_tokens(value: &str) -> usize {
    value.chars().count().saturating_add(3) / 4
}
