use std::path::PathBuf;

use opticcode_llm::ProviderId;
use serde::{Deserialize, Serialize};

use crate::EDIT_PLAN_SCHEMA_VERSION;

pub const MAX_EDIT_FILES: usize = 5;
pub const MAX_EDIT_CREATED_FILES: usize = 1;
pub const MAX_EDIT_FILE_BYTES: usize = 512 * 1024;
pub const MAX_EDIT_PROPOSAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EDIT_DIFF_DISPLAY_BYTES: usize = 1024 * 1024;
pub const MAX_EDIT_HUNKS: usize = 64;
pub const MAX_EDIT_ADDED_LINES: usize = 1_500;
pub const MAX_EDIT_DELETED_LINES: usize = 1_500;
pub const MAX_EDIT_CHANGED_LINES: usize = 2_000;
pub const MAX_EDIT_GLOBAL_TIMEOUT_SECONDS: u64 = 15 * 60;
pub const DEFAULT_PROPOSAL_TTL_SECONDS: u64 = 24 * 60 * 60;
pub const DEFAULT_TRANSACTION_REPORT_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
pub const MAX_EDIT_IDENTIFIER_BYTES: usize = 160;
pub const MAX_EDIT_REASON_CHARS: usize = 2_048;
pub const MAX_EDIT_LIST_ITEMS: usize = 64;

pub const ALLOWED_EDIT_EXTENSIONS: &[&str] = &[
    "java",
    "xml",
    "yml",
    "yaml",
    "json",
    "toml",
    "properties",
    "gradle",
    "md",
    "txt",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextEncoding {
    Utf8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    None,
    Lf,
    Crlf,
}

impl LineEnding {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lf => "lf",
            Self::Crlf => "crlf",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditContextReference {
    pub source: String,
    pub provenance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditUserReference {
    pub reference_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditValidationKind {
    ReparseJava,
    BuildOffline,
    TestOffline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditPlanLimits {
    pub max_files: usize,
    pub max_created_files: usize,
    pub max_file_bytes: usize,
    pub max_proposal_bytes: usize,
    pub max_hunks: usize,
    pub max_added_lines: usize,
    pub max_deleted_lines: usize,
    pub max_changed_lines: usize,
    pub global_timeout_seconds: u64,
}

impl Default for EditPlanLimits {
    fn default() -> Self {
        Self::hard_maxima()
    }
}

impl EditPlanLimits {
    pub const fn hard_maxima() -> Self {
        Self {
            max_files: MAX_EDIT_FILES,
            max_created_files: MAX_EDIT_CREATED_FILES,
            max_file_bytes: MAX_EDIT_FILE_BYTES,
            max_proposal_bytes: MAX_EDIT_PROPOSAL_BYTES,
            max_hunks: MAX_EDIT_HUNKS,
            max_added_lines: MAX_EDIT_ADDED_LINES,
            max_deleted_lines: MAX_EDIT_DELETED_LINES,
            max_changed_lines: MAX_EDIT_CHANGED_LINES,
            global_timeout_seconds: MAX_EDIT_GLOBAL_TIMEOUT_SECONDS,
        }
    }

    pub fn bounded_by_hard_maxima(self) -> bool {
        self.max_files > 0
            && self.max_files <= MAX_EDIT_FILES
            && self.max_created_files <= MAX_EDIT_CREATED_FILES
            && self.max_file_bytes > 0
            && self.max_file_bytes <= MAX_EDIT_FILE_BYTES
            && self.max_proposal_bytes > 0
            && self.max_proposal_bytes <= MAX_EDIT_PROPOSAL_BYTES
            && self.max_hunks > 0
            && self.max_hunks <= MAX_EDIT_HUNKS
            && self.max_added_lines <= MAX_EDIT_ADDED_LINES
            && self.max_deleted_lines <= MAX_EDIT_DELETED_LINES
            && self.max_changed_lines <= MAX_EDIT_CHANGED_LINES
            && self.global_timeout_seconds > 0
            && self.global_timeout_seconds <= MAX_EDIT_GLOBAL_TIMEOUT_SECONDS
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditOperation {
    Modify {
        path: String,
        expected_file_hash: String,
        encoding: TextEncoding,
        line_ending: LineEnding,
        range: ByteRange,
        expected_old: String,
        replacement: String,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbol: Option<String>,
        provenance: Vec<String>,
    },
    Create {
        path: String,
        extension: String,
        encoding: TextEncoding,
        line_ending: LineEnding,
        content: String,
        reason: String,
        provenance: Vec<String>,
        expected_absent: bool,
        declared_size: usize,
    },
}

impl EditOperation {
    pub fn path(&self) -> &str {
        match self {
            Self::Modify { path, .. } | Self::Create { path, .. } => path,
        }
    }

    pub const fn is_creation(&self) -> bool {
        matches!(self, Self::Create { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditPlan {
    pub schema_version: u32,
    pub plan_id: String,
    pub request_id: String,
    pub workspace_id: String,
    pub workspace_root_hash: String,
    pub profile: String,
    pub provider: ProviderId,
    pub model: String,
    pub base_head: String,
    pub working_tree_digest: String,
    pub context_used: Vec<EditContextReference>,
    pub user_references: Vec<EditUserReference>,
    pub summary: String,
    pub rationale_summary: String,
    pub operations: Vec<EditOperation>,
    pub validations: Vec<EditValidationKind>,
    pub risks: Vec<String>,
    pub limitations: Vec<String>,
    pub limits: EditPlanLimits,
    pub expires_at_unix_ms: u64,
}

impl EditPlan {
    pub fn new_empty() -> Self {
        Self {
            schema_version: EDIT_PLAN_SCHEMA_VERSION,
            plan_id: String::new(),
            request_id: String::new(),
            workspace_id: String::new(),
            workspace_root_hash: String::new(),
            profile: String::new(),
            provider: ProviderId::Ollama,
            model: String::new(),
            base_head: String::new(),
            working_tree_digest: String::new(),
            context_used: Vec::new(),
            user_references: Vec::new(),
            summary: String::new(),
            rationale_summary: String::new(),
            operations: Vec::new(),
            validations: Vec::new(),
            risks: Vec::new(),
            limitations: Vec::new(),
            limits: EditPlanLimits::default(),
            expires_at_unix_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPlanExpectations {
    pub request_id: String,
    pub plan_id: String,
    pub workspace_id: String,
    pub workspace_root: PathBuf,
    pub workspace_root_hash: String,
    pub profile: String,
    pub provider: ProviderId,
    pub model: String,
    pub base_head: String,
    pub working_tree_digest: String,
    pub now_unix_ms: u64,
    pub limits: EditPlanLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalFileStatus {
    Modified,
    Created,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalFileSnapshot {
    pub path: String,
    pub status: ProposalFileStatus,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_hash: Option<String>,
    pub proposed_content: String,
    pub proposed_hash: String,
    pub proposed_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedEditPlan {
    pub plan: EditPlan,
    pub files: Vec<ProposalFileSnapshot>,
    pub estimated_added_lines: usize,
    pub estimated_deleted_lines: usize,
    pub total_snapshot_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    Generated,
    Validated,
    WorktreePrepared,
    WorktreeApplied,
    BuildRunning,
    Verified,
    VerificationFailed,
    ApprovalPending,
    Applying,
    Applied,
    RollbackAvailable,
    RollingBack,
    RolledBack,
    Discarded,
    Expired,
    Failed,
}

impl ProposalState {
    pub fn can_transition_to(self, next: Self) -> bool {
        use ProposalState::*;
        matches!(
            (self, next),
            (Generated, Validated)
                | (
                    Validated,
                    WorktreePrepared | VerificationFailed | Failed | Expired | Discarded
                )
                | (
                    WorktreePrepared,
                    WorktreeApplied | VerificationFailed | Failed
                )
                | (WorktreeApplied, BuildRunning | VerificationFailed | Failed)
                | (BuildRunning, Verified | VerificationFailed | Failed)
                | (
                    Verified,
                    WorktreePrepared
                        | VerificationFailed
                        | ApprovalPending
                        | Applying
                        | Expired
                        | Discarded
                )
                | (VerificationFailed, WorktreePrepared | Expired | Discarded)
                | (ApprovalPending, Applying | Verified | Expired | Discarded)
                | (Applying, Applied | Verified | Failed)
                | (Applied, RollbackAvailable | RollingBack | Failed)
                | (RollbackAvailable, RollingBack | Expired)
                | (RollingBack, RolledBack | RollbackAvailable | Failed)
        ) || self == next
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::RolledBack | Self::Discarded | Self::Expired | Self::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalTransition {
    pub sequence: u32,
    pub state: ProposalState,
    pub recorded_at_unix_ms: u64,
    pub reason: String,
}
