use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{POLICY_PROTOCOL_ID, POLICY_SCHEMA_VERSION};

pub const MAX_POLICY_REQUEST_ID_BYTES: usize = 128;
pub const MAX_POLICY_IDENTIFIER_BYTES: usize = 160;
pub const MAX_POLICY_ACTIONS: usize = 128;
pub const MAX_POLICY_PATHS: usize = 512;
pub const MAX_POLICY_ARGUMENTS: usize = 256;
pub const MAX_POLICY_ARGUMENT_BYTES: usize = 8 * 1024;
pub const MAX_POLICY_ENVIRONMENT_KEYS: usize = 32;
pub const MAX_POLICY_TIMEOUT_MS: u64 = 60 * 60 * 1_000;
pub const MAX_POLICY_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    ReadOnly,
    WorktreeEdit,
    ApprovedApply,
}

impl PolicyMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorktreeEdit => "worktree_edit",
            Self::ApprovedApply => "approved_apply",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOrigin {
    User,
    Chat,
    Extension,
    Cli,
    Recovery,
    Model,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyClient {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyWorkspace {
    pub workspace_id: String,
    pub root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<GitRepositoryBoundary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_worktree: Option<ActiveWorktree>,
    /// Digest captured by the trusted Git-state adapter immediately before authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_tree_digest: Option<String>,
    /// Cleanliness captured by the trusted Git-state adapter at the same observation point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_clean: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitRepositoryBoundary {
    pub worktree_root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub index: PathBuf,
    pub object_dir: PathBuf,
    pub head: String,
    pub main_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveWorktree {
    pub run_id: String,
    pub owner_workspace_id: String,
    pub owner_request_id: String,
    pub root: PathBuf,
    pub source_root: PathBuf,
    pub base_head: String,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRange {
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathTarget {
    pub root: PathBuf,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TextRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchAction {
    pub root: PathBuf,
    pub query_hash: String,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextAction {
    pub root: PathBuf,
    pub task_hash: String,
    #[serde(default)]
    pub candidate_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RagAction {
    pub index_root: PathBuf,
    #[serde(default)]
    pub source_ids: Vec<String>,
    pub query_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteAction {
    pub target: PathTarget,
    pub recursive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchAction {
    pub root: PathBuf,
    pub paths: Vec<PathBuf>,
    #[serde(default)]
    pub created_paths: Vec<PathBuf>,
    pub diff_hash: String,
    pub files_hash: String,
    pub transaction_id: String,
    pub base_head: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkIntent {
    Denied,
    Declared,
    Required,
    Undeclared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessLaunch {
    Direct,
    WindowsCommandScript,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProcessAction {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_ms: u64,
    pub output_limit_bytes: usize,
    pub network: NetworkIntent,
    pub launch: ProcessLaunch,
    /// Names of variables copied from a sanitized host environment; values are never supplied by
    /// an untrusted action.
    #[serde(default)]
    pub environment_allowlist: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorktreeAction {
    pub repository_root: PathBuf,
    pub destination: PathBuf,
    pub base_head: String,
    pub run_id: String,
    pub detached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupWorktreeAction {
    pub repository_root: PathBuf,
    pub worktree_root: PathBuf,
    pub run_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitReadOperation {
    Status,
    Diff,
    Log,
    Show,
    RevParse,
    ListFiles,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitWriteOperation {
    Add,
    Restore,
    Checkout,
    Merge,
    WorktreeAdd,
    WorktreeRemove,
    Tag,
    Branch,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitReadAction {
    pub repository_root: PathBuf,
    pub operation: GitReadOperation,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitWriteAction {
    pub repository_root: PathBuf,
    pub operation: GitWriteOperation,
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitCommitAction {
    pub repository_root: PathBuf,
    pub tree_hash: String,
    pub message_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionAction {
    pub workspace_root: PathBuf,
    pub transaction_id: String,
    pub expected_state_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PolicyAction {
    ReadFile(PathTarget),
    ReadDirectory(PathTarget),
    Search(SearchAction),
    BuildContext(ContextAction),
    UseRag(RagAction),
    WriteFile(PathTarget),
    CreateFile(PathTarget),
    DeleteFile(DeleteAction),
    ApplyPatch(ApplyPatchAction),
    RunProcess(RunProcessAction),
    CreateWorktree(CreateWorktreeAction),
    CleanupWorktree(CleanupWorktreeAction),
    GitRead(GitReadAction),
    GitWrite(GitWriteAction),
    GitCommit(GitCommitAction),
    GitPush(GitWriteAction),
    NetworkAccess {
        destination_hash: String,
        purpose: String,
    },
    PackageInstall {
        ecosystem: String,
        package_set_hash: String,
    },
    Publish {
        target: String,
        artifact_hash: String,
    },
    RecoverTransaction(TransactionAction),
    RollbackTransaction(TransactionAction),
    ModifyPolicy,
    ElevatePrivileges,
    #[serde(other)]
    Unknown,
}

impl PolicyAction {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ReadFile(_) => "read_file",
            Self::ReadDirectory(_) => "read_directory",
            Self::Search(_) => "search",
            Self::BuildContext(_) => "build_context",
            Self::UseRag(_) => "use_rag",
            Self::WriteFile(_) => "write_file",
            Self::CreateFile(_) => "create_file",
            Self::DeleteFile(_) => "delete_file",
            Self::ApplyPatch(_) => "apply_patch",
            Self::RunProcess(_) => "run_process",
            Self::CreateWorktree(_) => "create_worktree",
            Self::CleanupWorktree(_) => "cleanup_worktree",
            Self::GitRead(_) => "git_read",
            Self::GitWrite(_) => "git_write",
            Self::GitCommit(_) => "git_commit",
            Self::GitPush(_) => "git_push",
            Self::NetworkAccess { .. } => "network_access",
            Self::PackageInstall { .. } => "package_install",
            Self::Publish { .. } => "publish",
            Self::RecoverTransaction(_) => "recover_transaction",
            Self::RollbackTransaction(_) => "rollback_transaction",
            Self::ModifyPolicy => "modify_policy",
            Self::ElevatePrivileges => "elevate_privileges",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRequest {
    pub schema_version: u32,
    pub protocol: String,
    pub request_id: String,
    pub action_id: String,
    pub origin: ActionOrigin,
    pub profile: String,
    pub client: PolicyClient,
    pub mode: PolicyMode,
    pub workspace: PolicyWorkspace,
    pub action: PolicyAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
}

impl PolicyRequest {
    pub fn new(
        request_id: impl Into<String>,
        action_id: impl Into<String>,
        mode: PolicyMode,
        workspace: PolicyWorkspace,
        action: PolicyAction,
    ) -> Self {
        Self {
            schema_version: POLICY_SCHEMA_VERSION,
            protocol: POLICY_PROTOCOL_ID.to_string(),
            request_id: request_id.into(),
            action_id: action_id.into(),
            origin: ActionOrigin::Cli,
            profile: "default".to_string(),
            client: PolicyClient {
                name: "opticcode-cli".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            mode,
            workspace,
            action,
            approval_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow {
        rule_id: String,
        risk: RiskLevel,
        scope: String,
        conditions: Vec<String>,
    },
    RequireApproval {
        rule_id: String,
        reason: String,
        risk: RiskLevel,
        summary: String,
    },
    Deny {
        rule_id: String,
        reason: String,
        risk: RiskLevel,
    },
}

impl PolicyDecision {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Allow { .. } => "allow",
            Self::RequireApproval { .. } => "require_approval",
            Self::Deny { .. } => "deny",
        }
    }

    pub fn rule_id(&self) -> &str {
        match self {
            Self::Allow { rule_id, .. }
            | Self::RequireApproval { rule_id, .. }
            | Self::Deny { rule_id, .. } => rule_id,
        }
    }

    pub const fn risk(&self) -> RiskLevel {
        match self {
            Self::Allow { risk, .. }
            | Self::RequireApproval { risk, .. }
            | Self::Deny { risk, .. } => *risk,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyReport {
    pub schema_version: u32,
    pub protocol: String,
    pub policy_version: String,
    pub request_id: String,
    pub action_id: String,
    pub action_kind: String,
    pub action_hash: String,
    pub workspace_hash: String,
    pub decision: PolicyDecision,
    pub user_reason: String,
    pub technical_reason: String,
    pub recommended_action: String,
    pub retriable: bool,
    pub revalidation_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_event_id: Option<String>,
}

impl PolicyReport {
    pub fn allowed(&self) -> bool {
        matches!(self.decision, PolicyDecision::Allow { .. })
    }
}
