use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const EVAL_SCHEMA_VERSION: u32 = 1;
pub const EVAL_SUITE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalSuite {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub description: String,
    pub fixtures: BTreeMap<String, EvalFixture>,
    pub cases: Vec<EvalCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvalFixture {
    Versioned {
        path: String,
    },
    External {
        external_id: String,
        description: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalCase {
    pub id: String,
    pub category: EvalCategory,
    pub prompt: String,
    pub fixture: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_query: Option<String>,
    #[serde(default)]
    pub expected: EvalExpected,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<EvalValidation>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub context_budget: EvalContextBudget,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EvalCategory {
    ExactSymbol,
    InterFileArchitecture,
    ChangeImpactCallers,
    ProjectConfiguration,
    LegacyAndNegative,
}

impl EvalCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactSymbol => "exact_symbol",
            Self::InterFileArchitecture => "inter_file_architecture",
            Self::ChangeImpactCallers => "change_impact_callers",
            Self::ProjectConfiguration => "project_configuration",
            Self::LegacyAndNegative => "legacy_and_negative",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalExpected {
    #[serde(default)]
    pub relevant_files: Vec<String>,
    #[serde(default)]
    pub relevant_symbols: Vec<String>,
    #[serde(default)]
    pub irrelevant_files: Vec<String>,
    #[serde(default)]
    pub facts: Vec<String>,
    #[serde(default)]
    pub forbidden_claims: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalValidation {
    pub kind: EvalValidationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub expected_files_unchanged: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvalValidationKind {
    ReadOnly,
    JavaSyntax,
    Build,
    Tests,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalContextBudget {
    pub max_files: usize,
    pub max_snippets: usize,
    pub max_chars: usize,
    pub max_estimated_tokens: usize,
}

impl Default for EvalContextBudget {
    fn default() -> Self {
        Self {
            max_files: 12,
            max_snippets: 12,
            max_chars: 24 * 1024,
            max_estimated_tokens: 6 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EvalStrategy {
    Legacy,
    Symbol,
    Exact,
    Rag,
}

impl EvalStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Symbol => "symbol",
            Self::Exact => "exact",
            Self::Rag => "rag",
        }
    }
}

impl std::fmt::Display for EvalStrategy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for EvalStrategy {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "legacy" => Ok(Self::Legacy),
            "symbol" => Ok(Self::Symbol),
            "exact" => Ok(Self::Exact),
            "rag" => Ok(Self::Rag),
            _ => Err(format!(
                "unsupported evaluation strategy `{value}`; expected legacy, symbol, exact, or rag"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvalLlmMode {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalConfiguration {
    pub strategies: Vec<EvalStrategy>,
    pub repetitions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_limit: Option<usize>,
    pub suite_truncated: bool,
    pub llm_mode: EvalLlmMode,
    pub context: EvalContextConfiguration,
    pub rag: EvalRagConfiguration,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<EvalGenerationConfiguration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalContextConfiguration {
    pub legacy_max_files: usize,
    pub legacy_max_bytes_per_file: usize,
    pub symbol_max_files: usize,
    pub symbol_max_snippets: usize,
    pub symbol_max_chars: usize,
    pub symbol_max_estimated_tokens: usize,
    pub exact_limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalRagConfiguration {
    pub enabled: bool,
    pub index_label: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalGenerationConfiguration {
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    pub max_generated_tokens: u32,
    pub http_timeout_ms: u64,
    pub warmup_runs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalEnvironment {
    pub os: String,
    pub arch: String,
    pub family: String,
    pub logical_cpus: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalRagIdentity {
    pub schema_version: u32,
    pub generation_id: String,
    pub configuration_hash: String,
    pub manifest_blake3: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvalCaseStatus {
    Completed,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalRetrievedItem {
    pub rank: usize,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub source: String,
    pub bytes: usize,
    pub chars: usize,
    pub estimated_tokens: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EvalObserved {
    pub retrieved: Vec<EvalRetrievedItem>,
    pub selected_files: Vec<String>,
    pub selected_symbols: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EvalRetrievalMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_at_1: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_at_3: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_at_5: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recall_at_1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recall_at_3: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recall_at_5: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recall_at_k: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reciprocal_rank: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ndcg_at_5: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_relevant_rank: Option<usize>,
    pub relevant_expected: usize,
    pub relevant_found_at_k: usize,
    pub duplicates: usize,
    pub unique_files: usize,
    pub file_diversity: f64,
    pub out_of_scope_results: usize,
    pub result_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalContextMetrics {
    pub files: usize,
    pub snippets: usize,
    pub chars: usize,
    pub bytes: usize,
    pub estimated_tokens: usize,
    pub token_estimator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_tokens: Option<u64>,
    pub expected_files_present: usize,
    pub expected_symbols_present: usize,
    pub irrelevant_files_present: usize,
    pub truncated_snippets: usize,
    pub budget_reached: bool,
    pub analysis_complete: bool,
    pub discovery_us: u64,
    pub ranking_us: u64,
    pub materialization_us: u64,
    pub total_us: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvalHumanReview {
    #[default]
    NotRequired,
    PendingHumanReview,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EvalResponseMetrics {
    pub generated: bool,
    pub expected_facts_found: usize,
    pub expected_facts_total: usize,
    pub forbidden_claims_found: usize,
    pub referenced_expected_files: usize,
    pub referenced_expected_symbols: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deterministic_quality_score: Option<f64>,
    pub build_validated: bool,
    pub tests_validated: bool,
    pub ast_validated: bool,
    pub scope_preserved: bool,
    pub human_review: EvalHumanReview,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental_llm_judge_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalCaseMetrics {
    pub retrieval: EvalRetrievalMetrics,
    pub context: EvalContextMetrics,
    pub response: EvalResponseMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalCaseResult {
    pub case_id: String,
    pub category: EvalCategory,
    pub fixture: String,
    pub strategy: EvalStrategy,
    pub repetition: u32,
    pub status: EvalCaseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub observed: EvalObserved,
    pub metrics: EvalCaseMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalStrategySummary {
    pub strategy: EvalStrategy,
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_at_1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_at_3: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_at_5: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_recall_at_k: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_reciprocal_rank: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_ndcg_at_5: Option<f64>,
    pub mean_estimated_tokens: f64,
    pub latency_p50_us: u64,
    pub latency_p95_us: u64,
    pub analysis_complete_rate: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EvalSummary {
    pub case_count: usize,
    pub execution_count: usize,
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub strategies: Vec<EvalStrategySummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EvalRegressionSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalRegression {
    pub strategy: EvalStrategy,
    pub metric: String,
    pub baseline: f64,
    pub candidate: f64,
    pub delta: f64,
    pub severity: EvalRegressionSeverity,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EvalBaselineComparison {
    pub baseline_run_id: String,
    pub candidate_run_id: String,
    pub comparable: bool,
    pub regressions: Vec<EvalRegression>,
    pub improvements: Vec<EvalRegression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalRunReport {
    pub schema_version: u32,
    pub run_id: String,
    pub opticcode_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    pub suite_id: String,
    pub suite_version: String,
    pub configuration: EvalConfiguration,
    pub configuration_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rag_identity: Option<EvalRagIdentity>,
    pub started_at_unix_ms: u64,
    pub duration_us: u64,
    pub environment: EvalEnvironment,
    pub results: Vec<EvalCaseResult>,
    pub summary: EvalSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<EvalBaselineComparison>,
    pub warnings: Vec<String>,
}
