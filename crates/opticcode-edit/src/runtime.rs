use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::MAX_EDIT_GLOBAL_TIMEOUT_SECONDS;

#[derive(Debug, Clone)]
pub struct EditRuntimeOptions {
    pub workspace_root: PathBuf,
    pub workspace_id: String,
    pub request_id: String,
    pub profile: String,
    pub client_name: String,
    pub client_version: String,
    pub git_timeout: Duration,
    pub build_timeout: Duration,
    pub output_limit_bytes: usize,
}

impl EditRuntimeOptions {
    pub fn new(
        workspace_root: impl Into<PathBuf>,
        workspace_id: impl Into<String>,
        request_id: impl Into<String>,
        profile: impl Into<String>,
    ) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            workspace_id: workspace_id.into(),
            request_id: request_id.into(),
            profile: profile.into(),
            client_name: "opticcode-runtime".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            git_timeout: Duration::from_secs(3 * 60),
            build_timeout: Duration::from_secs(MAX_EDIT_GLOBAL_TIMEOUT_SECONDS),
            output_limit_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditStageStatus {
    NotRun,
    Running,
    Passed,
    Failed,
    Cancelled,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditStageReport {
    pub status: EditStageStatus,
    pub duration_ms: u64,
    pub summary: String,
    pub errors: Vec<String>,
}

impl EditStageReport {
    pub fn passed(summary: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            status: EditStageStatus::Passed,
            duration_ms,
            summary: summary.into(),
            errors: Vec::new(),
        }
    }

    pub fn failed(summary: impl Into<String>, duration_ms: u64, error: impl Into<String>) -> Self {
        Self {
            status: EditStageStatus::Failed,
            duration_ms,
            summary: summary.into(),
            errors: vec![error.into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDecisionRecord {
    pub stage: String,
    pub action_kind: String,
    pub decision: String,
    pub rule_id: String,
    pub action_hash: String,
    pub audit_event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditProcessReport {
    pub tool: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub cancelled: bool,
    pub output_truncated: bool,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatEditVerificationReport {
    pub schema_version: u32,
    pub request_id: String,
    pub proposal_id: String,
    pub base_head: String,
    pub working_tree_digest: String,
    pub worktree_run_id: Option<String>,
    pub success: bool,
    pub source_unchanged: bool,
    pub lease_recovery_required: bool,
    pub worktree: EditStageReport,
    pub apply: EditStageReport,
    pub reparse: EditStageReport,
    pub build: EditStageReport,
    pub tests: EditStageReport,
    pub diff: EditStageReport,
    pub cleanup: EditStageReport,
    pub processes: Vec<EditProcessReport>,
    pub policy: Vec<PolicyDecisionRecord>,
    pub verified_at_unix_ms: Option<u64>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatEditApplyReport {
    pub schema_version: u32,
    pub request_id: String,
    pub proposal_id: String,
    pub transaction_id: String,
    pub success: bool,
    pub approval_id: String,
    pub approval_consumed: bool,
    pub post_reparse: EditStageReport,
    pub post_build: EditStageReport,
    pub rollback_attempted: bool,
    pub rollback_success: Option<bool>,
    pub policy: Vec<PolicyDecisionRecord>,
    pub applied_at_unix_ms: Option<u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatEditRollbackReport {
    pub schema_version: u32,
    pub request_id: String,
    pub proposal_id: String,
    pub transaction_id: String,
    pub success: bool,
    pub already_rolled_back: bool,
    pub approval_id: String,
    pub approval_consumed: bool,
    pub reparse: EditStageReport,
    pub policy: Vec<PolicyDecisionRecord>,
    pub rolled_back_at_unix_ms: Option<u64>,
    pub errors: Vec<String>,
}
