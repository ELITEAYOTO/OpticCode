use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Result};
use opticcode_tools::java_context::{
    build_java_task_context, JavaTaskContextOptions, JavaTaskContextReport,
};
use opticcode_tools::{build_project_context, FileSnippet, ProjectContext};
use serde::Serialize;

pub const ASSISTANT_CONTEXT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    Legacy,
    Symbol,
    Compare,
}

impl ContextMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Symbol => "symbol",
            Self::Compare => "compare",
        }
    }
}

impl std::fmt::Display for ContextMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for ContextMode {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "legacy" => Ok(Self::Legacy),
            "symbol" => Ok(Self::Symbol),
            "compare" => Ok(Self::Compare),
            _ => Err(format!(
                "unsupported context mode `{value}`; expected legacy, symbol, or compare"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextFallbackPolicy {
    Legacy,
    Refuse,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ContextRejectionReason {
    UnsupportedProject,
    IncompleteAnalysis,
    CriticalLimitReached,
    AmbiguousPrimarySymbol,
    SourceDrift,
    NoRelevantSymbol,
}

impl ContextRejectionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedProject => "unsupported_project",
            Self::IncompleteAnalysis => "incomplete_analysis",
            Self::CriticalLimitReached => "critical_limit_reached",
            Self::AmbiguousPrimarySymbol => "ambiguous_primary_symbol",
            Self::SourceDrift => "source_drift",
            Self::NoRelevantSymbol => "no_relevant_symbol",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextFallback {
    pub applied: bool,
    pub from: ContextMode,
    pub to: ContextMode,
    pub reasons: Vec<ContextRejectionReason>,
    pub warning: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantContextFile {
    pub path: String,
    pub snippets: usize,
    pub max_score: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantContextSnippet {
    pub id: String,
    pub path: String,
    pub role: String,
    pub symbol_id: Option<String>,
    pub score: Option<u32>,
    pub reasons: Vec<String>,
    pub bytes: usize,
    pub chars: usize,
    pub estimated_tokens: usize,
    pub truncated: bool,
    pub content_hash: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AssistantContextTimings {
    pub discovery_us: u64,
    pub ranking_us: u64,
    pub materialization_us: u64,
    pub total_us: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextVariantReport {
    pub mode: ContextMode,
    pub strategy: String,
    pub usable_for_generation: bool,
    pub rejection_reasons: Vec<ContextRejectionReason>,
    pub analysis_complete: bool,
    pub selection_complete: bool,
    pub truncated: bool,
    pub primary_symbol: Option<String>,
    pub primary_symbols: Vec<String>,
    pub primary_ambiguous: bool,
    pub files: Vec<AssistantContextFile>,
    pub snippets: Vec<AssistantContextSnippet>,
    pub rendered_bytes: usize,
    pub rendered_chars: usize,
    pub estimated_tokens: usize,
    pub token_estimator: String,
    pub truncations: Vec<String>,
    pub ambiguities: Vec<String>,
    pub warnings: Vec<String>,
    pub timings: AssistantContextTimings,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextComparison {
    pub legacy_estimated_tokens: usize,
    pub symbol_estimated_tokens: usize,
    pub estimated_token_delta: i64,
    pub estimated_token_reduction_basis_points: i32,
    pub legacy_files: usize,
    pub symbol_files: usize,
    pub shared_files: usize,
    pub symbol_usable_for_generation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedContextVariant {
    pub report: ContextVariantReport,
    #[serde(skip)]
    pub prompt_context: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextPreparation {
    pub schema_version: u32,
    pub requested_mode: ContextMode,
    pub used_mode: Option<ContextMode>,
    pub fallback: Option<ContextFallback>,
    pub analysis_complete: bool,
    pub variants: Vec<PreparedContextVariant>,
    pub comparison: Option<ContextComparison>,
}

pub fn prepare_assistant_context(
    workspace: &Path,
    task: &str,
    requested_mode: ContextMode,
    fallback_policy: ContextFallbackPolicy,
) -> Result<ContextPreparation> {
    match requested_mode {
        ContextMode::Legacy => {
            let legacy = prepare_legacy(workspace)?;
            Ok(ContextPreparation {
                schema_version: ASSISTANT_CONTEXT_SCHEMA_VERSION,
                requested_mode,
                used_mode: Some(ContextMode::Legacy),
                fallback: None,
                analysis_complete: legacy.report.analysis_complete,
                variants: vec![legacy],
                comparison: None,
            })
        }
        ContextMode::Symbol => {
            let symbol = prepare_symbol(workspace, task)?;
            if symbol.report.usable_for_generation {
                return Ok(ContextPreparation {
                    schema_version: ASSISTANT_CONTEXT_SCHEMA_VERSION,
                    requested_mode,
                    used_mode: Some(ContextMode::Symbol),
                    fallback: None,
                    analysis_complete: symbol.report.analysis_complete,
                    variants: vec![symbol],
                    comparison: None,
                });
            }
            if fallback_policy == ContextFallbackPolicy::Refuse {
                return Ok(ContextPreparation {
                    schema_version: ASSISTANT_CONTEXT_SCHEMA_VERSION,
                    requested_mode,
                    used_mode: None,
                    fallback: None,
                    analysis_complete: false,
                    variants: vec![symbol],
                    comparison: None,
                });
            }
            let reasons = symbol.report.rejection_reasons.clone();
            let legacy = prepare_legacy(workspace)?;
            Ok(ContextPreparation {
                schema_version: ASSISTANT_CONTEXT_SCHEMA_VERSION,
                requested_mode,
                used_mode: Some(ContextMode::Legacy),
                fallback: Some(ContextFallback {
                    applied: true,
                    from: ContextMode::Symbol,
                    to: ContextMode::Legacy,
                    reasons: reasons.clone(),
                    warning: format!(
                        "symbol context was rejected ({}) and legacy context was selected explicitly",
                        format_rejection_reasons(&reasons)
                    ),
                }),
                analysis_complete: false,
                variants: vec![symbol, legacy],
                comparison: None,
            })
        }
        ContextMode::Compare => {
            let legacy = prepare_legacy(workspace)?;
            let symbol = prepare_symbol(workspace, task)?;
            let comparison = compare_contexts(&legacy, &symbol);
            let analysis_complete = symbol.report.analysis_complete;
            Ok(ContextPreparation {
                schema_version: ASSISTANT_CONTEXT_SCHEMA_VERSION,
                requested_mode,
                used_mode: Some(ContextMode::Compare),
                fallback: None,
                analysis_complete,
                variants: vec![legacy, symbol],
                comparison: Some(comparison),
            })
        }
    }
}

impl ContextPreparation {
    pub fn variant(&self, mode: ContextMode) -> Option<&PreparedContextVariant> {
        self.variants
            .iter()
            .find(|variant| variant.report.mode == mode)
    }

    pub fn selected_variant(&self) -> Result<&PreparedContextVariant> {
        let mode = self
            .used_mode
            .filter(|mode| *mode != ContextMode::Compare)
            .ok_or_else(|| anyhow::anyhow!("context preparation has no single generation mode"))?;
        let variant = self
            .variant(mode)
            .ok_or_else(|| anyhow::anyhow!("selected context variant is missing"))?;
        if !variant.report.usable_for_generation {
            bail!(
                "selected context variant `{mode}` is not usable for generation: {}",
                format_rejection_reasons(&variant.report.rejection_reasons)
            );
        }
        Ok(variant)
    }
}

fn prepare_legacy(workspace: &Path) -> Result<PreparedContextVariant> {
    let started = Instant::now();
    let context = build_project_context(workspace)?;
    let prompt_context = context.to_prompt_context();
    let snippets = context
        .snippets
        .iter()
        .enumerate()
        .map(|(index, snippet)| legacy_snippet(index, snippet))
        .collect::<Vec<_>>();
    let files = aggregate_files(&snippets);
    let truncated = snippets.iter().any(|snippet| snippet.truncated);
    let truncations = truncated
        .then(|| "legacy_file_content".to_string())
        .into_iter()
        .collect();
    let total_us = duration_us(started.elapsed());
    let report = ContextVariantReport {
        mode: ContextMode::Legacy,
        strategy: "legacy_file_priority_v1".to_string(),
        usable_for_generation: true,
        rejection_reasons: Vec::new(),
        analysis_complete: true,
        selection_complete: true,
        truncated,
        primary_symbol: None,
        primary_symbols: Vec::new(),
        primary_ambiguous: false,
        files,
        snippets,
        rendered_bytes: prompt_context.len(),
        rendered_chars: prompt_context.chars().count(),
        estimated_tokens: estimate_tokens(&prompt_context),
        token_estimator: "estimate:ceil_unicode_chars_div_4".to_string(),
        truncations,
        ambiguities: Vec::new(),
        warnings: legacy_warnings(&context),
        timings: AssistantContextTimings {
            discovery_us: total_us,
            total_us,
            ..AssistantContextTimings::default()
        },
    };
    Ok(PreparedContextVariant {
        report,
        prompt_context,
    })
}

fn prepare_symbol(workspace: &Path, task: &str) -> Result<PreparedContextVariant> {
    let report = build_java_task_context(workspace, task, JavaTaskContextOptions::default())?;
    let prompt_context = report.to_prompt_context();
    let rejection_reasons = symbol_rejection_reasons(&report);
    let snippets = report
        .snippets
        .iter()
        .map(|snippet| AssistantContextSnippet {
            id: snippet.id.clone(),
            path: portable_path(&snippet.file),
            role: snippet.role.as_str().to_string(),
            symbol_id: snippet.symbol_id.clone(),
            score: Some(snippet.score),
            reasons: snippet.selection_reasons.clone(),
            bytes: snippet.content_bytes,
            chars: snippet.content_chars,
            estimated_tokens: snippet.estimated_tokens,
            truncated: snippet.truncated,
            content_hash: snippet.content_hash.clone(),
        })
        .collect::<Vec<_>>();
    let files = aggregate_files(&snippets);
    let truncations = symbol_truncations(&report);
    let ambiguities = if report.primary_ambiguous {
        report.primary_symbols.clone()
    } else {
        Vec::new()
    };
    let variant_report = ContextVariantReport {
        mode: ContextMode::Symbol,
        strategy: "java_symbol_context_v1".to_string(),
        usable_for_generation: rejection_reasons.is_empty(),
        rejection_reasons,
        analysis_complete: report.analysis_complete,
        selection_complete: report.selection_complete,
        truncated: report.truncated,
        primary_symbol: report.primary_symbol.clone(),
        primary_symbols: report.primary_symbols.clone(),
        primary_ambiguous: report.primary_ambiguous,
        files,
        snippets,
        rendered_bytes: report.budget.rendered_bytes,
        rendered_chars: report.budget.rendered_chars,
        estimated_tokens: report.budget.estimated_tokens,
        token_estimator: format!("estimate:{}", report.budget.token_estimator),
        truncations,
        ambiguities,
        warnings: report.warnings.clone(),
        timings: AssistantContextTimings {
            discovery_us: report
                .timings
                .index_us
                .saturating_add(report.timings.query_us),
            ranking_us: report.timings.ranking_us,
            materialization_us: report.timings.snippets_us,
            total_us: report.timings.total_us,
        },
    };
    Ok(PreparedContextVariant {
        report: variant_report,
        prompt_context,
    })
}

fn symbol_rejection_reasons(report: &JavaTaskContextReport) -> Vec<ContextRejectionReason> {
    let mut reasons = BTreeSet::new();
    if report.index_source.discovered_files == 0 {
        reasons.insert(ContextRejectionReason::UnsupportedProject);
    }
    if report.counts.source_read_errors > 0 || report.counts.source_hash_mismatches > 0 {
        reasons.insert(ContextRejectionReason::SourceDrift);
    }
    if report.primary_ambiguous {
        reasons.insert(ContextRejectionReason::AmbiguousPrimarySymbol);
    }
    if report.candidates.is_empty()
        && !report
            .snippets
            .iter()
            .any(|snippet| snippet.symbol_id.is_none())
    {
        reasons.insert(ContextRejectionReason::NoRelevantSymbol);
    }
    if !report.analysis_complete {
        reasons.insert(ContextRejectionReason::IncompleteAnalysis);
    }
    if report.truncated || !report.selection_complete {
        reasons.insert(ContextRejectionReason::CriticalLimitReached);
    }
    reasons.into_iter().collect()
}

fn symbol_truncations(report: &JavaTaskContextReport) -> Vec<String> {
    let mut values = Vec::new();
    let truncation = &report.truncation;
    for (name, active) in [
        ("index", truncation.index),
        ("diagnostics", truncation.diagnostics),
        ("warnings", truncation.warnings),
        ("query", truncation.query),
        ("symbols", truncation.symbols),
        ("candidates", truncation.candidates),
        ("relations", truncation.relations),
        ("relation_depth", truncation.relation_depth),
        ("snippets", truncation.snippets),
        ("context_bytes", truncation.context_bytes),
        ("context_chars", truncation.context_chars),
        ("estimated_tokens", truncation.estimated_tokens),
    ] {
        if active {
            values.push(name.to_string());
        }
    }
    values
}

fn legacy_snippet(index: usize, snippet: &FileSnippet) -> AssistantContextSnippet {
    AssistantContextSnippet {
        id: format!("legacy-{:03}", index + 1),
        path: portable_path(&snippet.path),
        role: "legacy_file".to_string(),
        symbol_id: None,
        score: None,
        reasons: vec!["legacy_file_priority_v1".to_string()],
        bytes: snippet.content.len(),
        chars: snippet.content.chars().count(),
        estimated_tokens: estimate_tokens(&snippet.content),
        truncated: snippet.truncated,
        content_hash: blake3::hash(snippet.content.as_bytes())
            .to_hex()
            .to_string(),
    }
}

fn aggregate_files(snippets: &[AssistantContextSnippet]) -> Vec<AssistantContextFile> {
    let mut files = BTreeMap::<String, (usize, Option<u32>)>::new();
    for snippet in snippets {
        let entry = files.entry(snippet.path.clone()).or_insert((0, None));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = match (entry.1, snippet.score) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, score) | (score, None) => score,
        };
    }
    files
        .into_iter()
        .map(|(path, (snippets, max_score))| AssistantContextFile {
            path,
            snippets,
            max_score,
        })
        .collect()
}

fn compare_contexts(
    legacy: &PreparedContextVariant,
    symbol: &PreparedContextVariant,
) -> ContextComparison {
    let baseline = legacy.report.estimated_tokens;
    let selected = symbol.report.estimated_tokens;
    let delta = baseline as i64 - selected as i64;
    let reduction = if baseline == 0 {
        0
    } else {
        ((delta.saturating_mul(10_000)) / baseline as i64)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    let legacy_files = legacy
        .report
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let symbol_files = symbol
        .report
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    ContextComparison {
        legacy_estimated_tokens: baseline,
        symbol_estimated_tokens: selected,
        estimated_token_delta: delta,
        estimated_token_reduction_basis_points: reduction,
        legacy_files: legacy_files.len(),
        symbol_files: symbol_files.len(),
        shared_files: legacy_files.intersection(&symbol_files).count(),
        symbol_usable_for_generation: symbol.report.usable_for_generation,
    }
}

fn legacy_warnings(context: &ProjectContext) -> Vec<String> {
    let mut warnings = Vec::new();
    if context.report.total_files_seen > context.report.sampled_files.len() {
        warnings.push("legacy workspace inventory reached its sampling bound".to_string());
    }
    if context.snippets.iter().any(|snippet| snippet.truncated) {
        warnings.push("one or more legacy file snippets were truncated".to_string());
    }
    warnings
}

fn format_rejection_reasons(reasons: &[ContextRejectionReason]) -> String {
    if reasons.is_empty() {
        "none".to_string()
    } else {
        reasons
            .iter()
            .map(|reason| reason.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn portable_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn estimate_tokens(value: &str) -> usize {
    value.chars().count().saturating_add(3) / 4
}

fn duration_us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        prepare_assistant_context, ContextFallbackPolicy, ContextMode, ContextRejectionReason,
    };

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-index-mini")
    }

    #[test]
    fn legacy_mode_preserves_the_existing_selector() {
        let context = prepare_assistant_context(
            &fixture(),
            "Locate Helpers#ping().",
            ContextMode::Legacy,
            ContextFallbackPolicy::Legacy,
        )
        .unwrap();

        assert_eq!(context.used_mode, Some(ContextMode::Legacy));
        assert!(context.fallback.is_none());
        assert!(context.selected_variant().unwrap().prompt_context.len() > 100);
        assert_eq!(context.variants.len(), 1);
    }

    #[test]
    fn symbol_mode_is_used_only_for_complete_unambiguous_analysis() {
        let context = prepare_assistant_context(
            &fixture(),
            "Locate dev.opticcode.util.Helpers#ping().",
            ContextMode::Symbol,
            ContextFallbackPolicy::Legacy,
        )
        .unwrap();

        assert_eq!(context.used_mode, Some(ContextMode::Symbol));
        assert!(context.analysis_complete);
        assert!(context.fallback.is_none());
        assert!(context
            .selected_variant()
            .unwrap()
            .report
            .primary_symbols
            .iter()
            .any(|symbol| symbol.ends_with("Helpers#ping()")));
    }

    #[test]
    fn incomplete_symbol_analysis_falls_back_explicitly() {
        let context = prepare_assistant_context(
            &fixture(),
            "Inspect dev.opticcode.util.Helpers#create(String).",
            ContextMode::Symbol,
            ContextFallbackPolicy::Legacy,
        )
        .unwrap();

        assert_eq!(context.used_mode, Some(ContextMode::Legacy));
        let fallback = context.fallback.as_ref().unwrap();
        assert!(fallback.applied);
        assert!(fallback
            .reasons
            .contains(&ContextRejectionReason::IncompleteAnalysis));
        assert!(fallback
            .reasons
            .contains(&ContextRejectionReason::CriticalLimitReached));
        assert!(!context.analysis_complete);
    }

    #[test]
    fn strict_symbol_mode_refuses_ambiguous_primary_symbols() {
        let context = prepare_assistant_context(
            &fixture(),
            "Inspect Duplicate.",
            ContextMode::Symbol,
            ContextFallbackPolicy::Refuse,
        )
        .unwrap();

        assert_eq!(context.used_mode, None);
        assert!(context.fallback.is_none());
        let symbol = context.variant(ContextMode::Symbol).unwrap();
        assert!(symbol.report.primary_ambiguous);
        assert!(symbol
            .report
            .rejection_reasons
            .contains(&ContextRejectionReason::AmbiguousPrimarySymbol));
        assert!(context.selected_variant().is_err());
    }

    #[test]
    fn compare_mode_builds_both_contexts_without_selecting_one_for_generation() {
        let context = prepare_assistant_context(
            &fixture(),
            "Locate dev.opticcode.util.Helpers#ping().",
            ContextMode::Compare,
            ContextFallbackPolicy::Legacy,
        )
        .unwrap();

        assert_eq!(context.used_mode, Some(ContextMode::Compare));
        assert_eq!(context.variants.len(), 2);
        assert!(context.comparison.is_some());
        assert!(context.selected_variant().is_err());
    }
}
