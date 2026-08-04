use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;

use crate::approval::{
    ApprovalBinding, ApprovalError, ApprovalFileBinding, ApprovalGrant, ApprovalStore,
    NativeConfirmation,
};
use crate::audit::{unix_millis, AuditEvent, AuditStore};
use crate::model::{
    ActionOrigin, ActiveWorktree, ApplyPatchAction, CleanupWorktreeAction, GitReadOperation,
    GitRepositoryBoundary, GitWriteOperation, NetworkIntent, PathTarget, PolicyAction,
    PolicyDecision, PolicyMode, PolicyReport, PolicyRequest, ProcessLaunch, RiskLevel,
    RunProcessAction, MAX_POLICY_ARGUMENTS, MAX_POLICY_ARGUMENT_BYTES, MAX_POLICY_ENVIRONMENT_KEYS,
    MAX_POLICY_IDENTIFIER_BYTES, MAX_POLICY_OUTPUT_BYTES, MAX_POLICY_PATHS,
    MAX_POLICY_REQUEST_ID_BYTES, MAX_POLICY_TIMEOUT_MS,
};
use crate::paths::{
    inspect_path, inspect_root, revalidate, PathExpectation, PathSafetyError, PathSafetyReport,
};
use crate::{POLICY_PROTOCOL_ID, POLICY_SCHEMA_VERSION, POLICY_VERSION};

const MAX_TECHNICAL_REASON_BYTES: usize = 2 * 1024;
const MAX_USER_REASON_BYTES: usize = 512;
const WORKTREE_STORAGE_DIRECTORY: &str = "opticcode-worktrees";
const WORKTREE_RUNS_DIRECTORY: &str = "runs";
const WORKTREE_LEASES_DIRECTORY: &str = "leases";
const MAX_LEASE_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub enum PolicyError {
    InvalidRequest(String),
    Storage(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(formatter, "invalid policy request: {message}"),
            Self::Storage(message) => write!(formatter, "policy storage failure: {message}"),
        }
    }
}

impl std::error::Error for PolicyError {}

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    audit: AuditStore,
    approvals: ApprovalStore,
}

#[derive(Debug, Clone)]
pub struct PolicyPreflight {
    pub report: PolicyReport,
    pub paths: Vec<PathSafetyReport>,
    request: PolicyRequest,
    base_rule_id: String,
    security_context_hash: String,
}

impl PolicyPreflight {
    pub fn revalidate(&self) -> std::result::Result<(), PolicyError> {
        self.revalidate_observed(&self.request)
    }

    /// Revalidate against a request rebuilt from fresh Git/worktree observations immediately
    /// before execution. Executors must not reuse stale observed state for mutating actions.
    pub fn revalidate_observed(
        &self,
        observed: &PolicyRequest,
    ) -> std::result::Result<(), PolicyError> {
        if security_context_hash(observed)? != self.security_context_hash {
            return Err(PolicyError::InvalidRequest(
                "policy security context changed after authorization".to_string(),
            ));
        }
        let fresh = evaluate(observed)?;
        if fresh.report.decision.rule_id() != self.base_rule_id || fresh.paths != self.paths {
            return Err(PolicyError::InvalidRequest(
                "policy decision inputs changed after authorization".to_string(),
            ));
        }
        for before in &self.paths {
            let after = revalidate(before)
                .map_err(|error| PolicyError::InvalidRequest(error.to_string()))?;
            if before != &after {
                return Err(PolicyError::InvalidRequest(
                    "policy path changed after authorization".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Evaluation {
    report: PolicyReport,
    paths: Vec<PathSafetyReport>,
}

impl PolicyEngine {
    pub fn open(state_root: impl Into<PathBuf>) -> std::result::Result<Self, PolicyError> {
        let state_root = state_root.into();
        let audit = AuditStore::open(state_root.clone())
            .map_err(|error| PolicyError::Storage(error.to_string()))?;
        let approvals = ApprovalStore::open(state_root)
            .map_err(|error| PolicyError::Storage(error.to_string()))?;
        Ok(Self { audit, approvals })
    }

    pub fn default_engine() -> std::result::Result<Self, PolicyError> {
        let audit =
            AuditStore::default_store().map_err(|error| PolicyError::Storage(error.to_string()))?;
        let approvals = ApprovalStore::open(audit.state_root().to_path_buf())
            .map_err(|error| PolicyError::Storage(error.to_string()))?;
        Ok(Self { audit, approvals })
    }

    pub fn audit_store(&self) -> &AuditStore {
        &self.audit
    }

    pub fn explain(
        &self,
        request: &PolicyRequest,
    ) -> std::result::Result<PolicyPreflight, PolicyError> {
        self.ensure_state_outside_workspace(request)?;
        let evaluation = evaluate(request)?;
        let base_rule_id = evaluation.report.decision.rule_id().to_string();
        Ok(PolicyPreflight {
            report: evaluation.report,
            paths: evaluation.paths,
            request: request.clone(),
            base_rule_id,
            security_context_hash: security_context_hash(request)?,
        })
    }

    pub fn check(
        &self,
        request: &PolicyRequest,
    ) -> std::result::Result<PolicyPreflight, PolicyError> {
        self.ensure_state_outside_workspace(request)?;
        let started = Instant::now();
        let mut evaluation = evaluate(request)?;
        let base_rule_id = evaluation.report.decision.rule_id().to_string();
        let mut approval_status = if request.approval_id.is_some() {
            "present"
        } else {
            "none"
        };

        if matches!(
            evaluation.report.decision,
            PolicyDecision::RequireApproval { .. }
        ) {
            if let Some(approval_id) = request.approval_id.as_deref() {
                let binding = approval_binding(request, &evaluation.paths)?;
                match self.approvals.consume(approval_id, &binding) {
                    Ok(_) => {
                        approval_status = "consumed";
                        evaluation.report.decision = PolicyDecision::Allow {
                            rule_id: "approval.one_shot".to_string(),
                            risk: RiskLevel::Low,
                            scope: request.action.kind().to_string(),
                            conditions: vec![
                                "native confirmation".to_string(),
                                "one-shot approval consumed".to_string(),
                                "state binding matched".to_string(),
                                "revalidate immediately before execution".to_string(),
                            ],
                        };
                        evaluation.report.user_reason =
                            "The exact approved action may run once.".to_string();
                        evaluation.report.technical_reason =
                            "The runtime consumed a valid state-bound approval record.".to_string();
                        evaluation.report.recommended_action =
                            "Execute immediately, then verify postconditions.".to_string();
                        evaluation.report.retriable = false;
                    }
                    Err(error) => {
                        approval_status = "invalid";
                        apply_approval_failure(&mut evaluation.report, error);
                    }
                }
            }
        }

        let event = AuditEvent {
            schema_version: 1,
            event_id: String::new(),
            timestamp_unix_ms: unix_millis(),
            request_id: request.request_id.clone(),
            action_id_hash: hash_text(&request.action_id),
            action_kind: request.action.kind().to_string(),
            action_hash: evaluation.report.action_hash.clone(),
            rule_id: evaluation.report.decision.rule_id().to_string(),
            decision: evaluation.report.decision.kind().to_string(),
            risk: evaluation.report.decision.risk(),
            workspace_hash: evaluation.report.workspace_hash.clone(),
            origin: request.origin,
            approval_state: approval_status.to_string(),
            approval_hash: request.approval_id.as_deref().map(hash_text),
            transaction_hash: transaction_id(&request.action).map(hash_text),
            result: if evaluation.report.allowed() {
                "authorized"
            } else {
                "blocked"
            }
            .to_string(),
            duration_us: duration_us(started),
        };
        let event_id = self
            .audit
            .record(event)
            .map_err(|error| PolicyError::Storage(error.to_string()))?;
        evaluation.report.audit_event_id = Some(event_id);
        Ok(PolicyPreflight {
            report: evaluation.report,
            paths: evaluation.paths,
            request: request.clone(),
            base_rule_id,
            security_context_hash: security_context_hash(request)?,
        })
    }

    pub fn issue_approval(
        &self,
        request: &PolicyRequest,
        confirmation: &NativeConfirmation,
        ttl_seconds: u64,
    ) -> std::result::Result<ApprovalGrant, PolicyError> {
        self.ensure_state_outside_workspace(request)?;
        if request.approval_id.is_some() {
            return Err(PolicyError::InvalidRequest(
                "cannot issue an approval from a request that already carries one".to_string(),
            ));
        }
        let evaluation = evaluate(request)?;
        if !matches!(
            evaluation.report.decision,
            PolicyDecision::RequireApproval { .. }
        ) {
            return Err(PolicyError::InvalidRequest(
                "only require_approval decisions can receive a grant".to_string(),
            ));
        }
        let binding = approval_binding(request, &evaluation.paths)?;
        self.approvals
            .issue(binding, confirmation, ttl_seconds)
            .map_err(|error| PolicyError::Storage(error.to_string()))
    }

    pub fn approval_binding(
        &self,
        request: &PolicyRequest,
    ) -> std::result::Result<ApprovalBinding, PolicyError> {
        self.ensure_state_outside_workspace(request)?;
        let evaluation = evaluate(request)?;
        approval_binding(request, &evaluation.paths)
    }

    fn ensure_state_outside_workspace(
        &self,
        request: &PolicyRequest,
    ) -> std::result::Result<(), PolicyError> {
        let state = fs::canonicalize(self.audit.state_root())
            .map_err(|error| PolicyError::Storage(error.to_string()))?;
        let workspace = inspect_root(&request.workspace.root).map_err(invalid_path_request)?;
        if state.starts_with(&workspace) || workspace.starts_with(&state) {
            return Err(PolicyError::Storage(
                "policy state must not overlap the source workspace".to_string(),
            ));
        }
        Ok(())
    }
}

fn evaluate(request: &PolicyRequest) -> std::result::Result<Evaluation, PolicyError> {
    validate_request(request)?;
    let workspace_root = inspect_root(&request.workspace.root).map_err(invalid_path_request)?;
    let workspace_hash = hash_text(&format!(
        "{}:{}",
        request.workspace.workspace_id,
        normalized_absolute(&workspace_root)
    ));
    let action_hash = action_hash(&request.action)?;
    let mut paths = Vec::new();
    let outcome = evaluate_action(request, &workspace_root, &mut paths);
    let (decision, user_reason, technical_reason, recommended_action, retriable) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => denied(
            error.rule_id,
            RiskLevel::High,
            "The requested path is blocked by the safety policy.",
            &error.message,
            "Choose a regular, non-sensitive path inside the authorized workspace.",
            false,
        ),
    };
    Ok(Evaluation {
        report: PolicyReport {
            schema_version: POLICY_SCHEMA_VERSION,
            protocol: POLICY_PROTOCOL_ID.to_string(),
            policy_version: POLICY_VERSION.to_string(),
            request_id: request.request_id.clone(),
            action_id: request.action_id.clone(),
            action_kind: request.action.kind().to_string(),
            action_hash,
            workspace_hash,
            decision,
            user_reason: bounded(&user_reason, MAX_USER_REASON_BYTES),
            technical_reason: bounded(&technical_reason, MAX_TECHNICAL_REASON_BYTES),
            recommended_action: bounded(&recommended_action, MAX_USER_REASON_BYTES),
            retriable,
            revalidation_required: !paths.is_empty(),
            audit_event_id: None,
        },
        paths,
    })
}

type RuleOutcome = (PolicyDecision, String, String, String, bool);

fn evaluate_action(
    request: &PolicyRequest,
    workspace_root: &Path,
    paths: &mut Vec<PathSafetyReport>,
) -> Result<RuleOutcome, PathSafetyError> {
    match &request.action {
        PolicyAction::Unknown => Ok(denied(
            "action.unknown",
            RiskLevel::Critical,
            "Unknown actions are never authorized.",
            "The closed policy action enum did not recognize this action.",
            "Upgrade the client or use a supported structured action.",
            false,
        )),
        PolicyAction::ModifyPolicy => Ok(denied(
            "policy.modification_denied",
            RiskLevel::Critical,
            "The model cannot modify its authorization policy.",
            "Policy changes are outside all runtime action scopes.",
            "Change policy source manually and review it as code.",
            false,
        )),
        PolicyAction::ElevatePrivileges => Ok(denied(
            "process.elevation_denied",
            RiskLevel::Critical,
            "Privilege elevation is not available.",
            "Elevated execution is excluded from OpticCode runtime capabilities.",
            "Run any administrative maintenance manually outside OpticCode.",
            false,
        )),
        PolicyAction::ReadFile(target) => {
            let report = inspect_path(target, PathExpectation::File)?;
            require_read_scope(request, workspace_root, &report.root)?;
            paths.push(report);
            Ok(allowed(
                "read.safe_workspace_file",
                "workspace_source",
                &["regular path", "inside workspace", "secret path denied", "TOCTOU revalidation"],
                "Safe workspace source may be read.",
                "The file passed canonicalization, root, reparse, nested-repository, and sensitive-path checks.",
            ))
        }
        PolicyAction::ReadDirectory(target) => {
            let report = inspect_path(target, PathExpectation::Directory)?;
            require_read_scope(request, workspace_root, &report.root)?;
            paths.push(report);
            Ok(allowed(
                "read.safe_workspace_directory",
                "workspace_directory",
                &["regular path", "inside workspace", "bounded traversal required"],
                "The safe workspace directory may be inspected.",
                "Directory traversal remains bounded and cannot cross links or repository boundaries.",
            ))
        }
        PolicyAction::Search(action) => {
            let root = inspect_root(&action.root)?;
            require_read_scope(request, workspace_root, &root)?;
            validate_hash_value(&action.query_hash, "search query hash")?;
            inspect_path_list(&root, &action.paths, paths)?;
            Ok(allowed(
                "analysis.search_read_only",
                "workspace_search",
                &["bounded query", "safe source types", "no writes"],
                "Read-only workspace search is allowed.",
                "Search is constrained to the canonical workspace and safe paths.",
            ))
        }
        PolicyAction::BuildContext(action) => {
            let root = inspect_root(&action.root)?;
            require_read_scope(request, workspace_root, &root)?;
            validate_hash_value(&action.task_hash, "context task hash")?;
            inspect_path_list(&root, &action.candidate_paths, paths)?;
            Ok(allowed(
                "analysis.context_read_only",
                "workspace_context",
                &["bounded context", "safe source reads", "no writes"],
                "Java and project context may be built.",
                "Context construction is a read-only action over validated roots.",
            ))
        }
        PolicyAction::UseRag(action) => {
            validate_hash_value(&action.query_hash, "RAG query hash")?;
            let root = inspect_root(&action.index_root)?;
            if !root.starts_with(workspace_root) {
                return Ok(denied(
                    "rag.index_outside_workspace",
                    RiskLevel::High,
                    "The RAG index is outside the authorized workspace.",
                    "This policy version only trusts a workspace-owned validated index.",
                    "Use the configured OpticCode RAG v2 index inside the workspace.",
                    false,
                ));
            }
            Ok(allowed(
                "rag.safe_index_read",
                "rag_v2",
                &["validated index", "bounded hits", "secret exclusions"],
                "The validated local RAG index may be queried.",
                "RAG access is local and read-only; source policy still applies.",
            ))
        }
        PolicyAction::WriteFile(target) => evaluate_write(
            request,
            workspace_root,
            target,
            PathExpectation::File,
            false,
            paths,
        ),
        PolicyAction::CreateFile(target) => evaluate_write(
            request,
            workspace_root,
            target,
            PathExpectation::NewFile,
            true,
            paths,
        ),
        PolicyAction::DeleteFile(action) => {
            if action.recursive {
                return Ok(denied(
                    "delete.recursive_denied",
                    RiskLevel::Critical,
                    "Recursive deletion is not available.",
                    "The action requested a recursive filesystem delete.",
                    "Review and delete individual files manually if still necessary.",
                    false,
                ));
            }
            let report = inspect_path(&action.target, PathExpectation::File)?;
            require_workspace_or_active_root(request, workspace_root, &report.root)?;
            if request.mode == PolicyMode::ReadOnly {
                return Ok(read_only_write_denied());
            }
            if report.root == workspace_root {
                return Ok(denied(
                    "delete.original_requires_apply",
                    RiskLevel::Critical,
                    "Direct deletion from the original workspace is blocked.",
                    "Original deletions must be represented by a verified transactional patch.",
                    "Create and verify the deletion in a disposable worktree first.",
                    false,
                ));
            }
            paths.push(report);
            Ok(approval_required(
                "delete.worktree_file_requires_approval",
                RiskLevel::High,
                "Deleting a proposal file requires explicit confirmation.",
                "The deletion is confined to the disposable worktree but remains destructive.",
                "Review the exact worktree file and approve the one-shot action.",
            ))
        }
        PolicyAction::ApplyPatch(action) => {
            evaluate_apply_patch(request, workspace_root, action, paths)
        }
        PolicyAction::RunProcess(action) => {
            evaluate_process(request, workspace_root, action, paths)
        }
        PolicyAction::CreateWorktree(action) => {
            let repository = inspect_root(&action.repository_root)?;
            if repository != workspace_root {
                return Ok(outside_scope_denied());
            }
            validate_git_oid(&action.base_head, "worktree base HEAD")?;
            validate_worktree_destination(&action.destination, &action.run_id)?;
            if !action.detached {
                return Ok(denied(
                    "worktree.detached_required",
                    RiskLevel::High,
                    "OpticCode worktrees must use detached HEAD.",
                    "Creating or moving a user branch is outside the disposable-worktree scope.",
                    "Create the worktree at the exact base commit with detached HEAD.",
                    false,
                ));
            }
            if request.mode != PolicyMode::WorktreeEdit {
                return Ok(read_only_write_denied());
            }
            let Some(boundary) = request.workspace.repository.as_ref() else {
                return Ok(denied(
                    "worktree.git_boundary_required",
                    RiskLevel::High,
                    "Creating a worktree requires an observed Git boundary.",
                    "The source HEAD and repository layout were not supplied by the trusted Git adapter.",
                    "Refresh repository state before creating the disposable worktree.",
                    true,
                ));
            };
            if boundary.head != action.base_head {
                return Ok(denied(
                    "worktree.head_mismatch",
                    RiskLevel::High,
                    "The source HEAD changed before worktree creation.",
                    "The requested base commit differs from the observed repository boundary.",
                    "Refresh and recreate the proposal from the current HEAD.",
                    true,
                ));
            }
            if request.workspace.repository_clean != Some(true) {
                return Ok(denied(
                    "worktree.source_dirty",
                    RiskLevel::High,
                    "The source repository must be clean before worktree creation.",
                    "A missing or false cleanliness observation fails closed.",
                    "Commit, stash, or otherwise resolve source changes, then refresh Git state.",
                    true,
                ));
            }
            require_working_tree_digest(request)?;
            Ok(allowed(
                "worktree.create_disposable",
                "opticcode_temp_worktree",
                &[
                    "detached HEAD",
                    "controlled temporary root",
                    "exact base commit",
                    "recoverable lease",
                ],
                "A disposable OpticCode worktree may be created.",
                "The destination is a direct child of controlled temporary storage.",
            ))
        }
        PolicyAction::CleanupWorktree(action) => evaluate_cleanup(request, workspace_root, action),
        PolicyAction::GitRead(action) => {
            if action.operation == GitReadOperation::Unknown {
                return Ok(denied(
                    "git.read_operation_unknown",
                    RiskLevel::High,
                    "Unknown Git operations are blocked.",
                    "The Git read operation is outside the closed allowlist.",
                    "Use status, diff, log, show, rev-parse, or list-files.",
                    false,
                ));
            }
            let repository = inspect_root(&action.repository_root)?;
            require_workspace_or_active_root(request, workspace_root, &repository)?;
            inspect_path_list(&repository, &action.paths, paths)?;
            Ok(allowed(
                "git.read_allowlist",
                "repository_read",
                &["read-only Git operation", "no ref mutation", "no network"],
                "The requested Git inspection is allowed.",
                "The operation is in the closed read-only Git allowlist.",
            ))
        }
        PolicyAction::GitWrite(action) => {
            if action.operation == GitWriteOperation::Unknown {
                return Ok(denied(
                    "git.write_operation_unknown",
                    RiskLevel::Critical,
                    "Unknown Git writes are blocked.",
                    "The requested Git mutation is not represented by a safe dedicated action.",
                    "Use a dedicated worktree or transaction operation.",
                    false,
                ));
            }
            Ok(denied(
                "git.generic_write_denied",
                RiskLevel::High,
                "Generic Git writes are not available.",
                "Worktree and apply mutations must use their dedicated structured actions.",
                "Use CreateWorktree, CleanupWorktree, ApplyPatch, or GitCommit.",
                false,
            ))
        }
        PolicyAction::GitCommit(action) => {
            validate_hash_value(&action.tree_hash, "Git tree hash")?;
            validate_hash_value(&action.message_hash, "commit message hash")?;
            let repository = inspect_root(&action.repository_root)?;
            require_workspace_or_active_root(request, workspace_root, &repository)?;
            if request.mode == PolicyMode::ReadOnly {
                Ok(read_only_write_denied())
            } else {
                Ok(approval_required(
                    "git.local_commit_requires_approval",
                    RiskLevel::High,
                    "Creating a local commit requires explicit confirmation.",
                    "A commit changes repository refs even without network access.",
                    "Review the staged tree and approve the exact one-shot commit.",
                ))
            }
        }
        PolicyAction::GitPush(_) => Ok(denied(
            "git.push_denied",
            RiskLevel::Critical,
            "Git push is never performed by this policy version.",
            "Remote ref mutation is outside OpticCode local-agent capabilities.",
            "Push manually after reviewing local changes and commits.",
            false,
        )),
        PolicyAction::NetworkAccess { .. } => Ok(denied(
            "network.standalone_denied",
            RiskLevel::Critical,
            "Standalone network access is unavailable.",
            "Network access must be tied to an explicitly approved bounded operation.",
            "Use local data, or approve a supported Maven/Gradle verification action.",
            false,
        )),
        PolicyAction::PackageInstall { .. } => Ok(denied(
            "package.install_denied",
            RiskLevel::Critical,
            "Automatic package installation is disabled.",
            "Package installation changes the machine outside the project transaction boundary.",
            "Install reviewed dependencies manually.",
            false,
        )),
        PolicyAction::Publish { .. } => Ok(denied(
            "publish.denied",
            RiskLevel::Critical,
            "Publishing and releases are disabled.",
            "The action would expose or distribute an artifact outside the local workspace.",
            "Publish manually after reviewing the verified artifact.",
            false,
        )),
        PolicyAction::RecoverTransaction(action) => {
            validate_transaction_action(action, workspace_root)?;
            if request.mode == PolicyMode::ReadOnly {
                return Ok(read_only_write_denied());
            }
            Ok(approval_required(
                "transaction.recovery_requires_approval",
                RiskLevel::High,
                "Targeted recovery requires explicit confirmation.",
                "Recovery may mutate files or remove abandoned controlled resources.",
                "Inspect the transaction, then approve this exact one-shot recovery.",
            ))
        }
        PolicyAction::RollbackTransaction(action) => {
            validate_transaction_action(action, workspace_root)?;
            if request.mode == PolicyMode::ReadOnly {
                return Ok(read_only_write_denied());
            }
            Ok(approval_required(
                "transaction.rollback_requires_approval",
                RiskLevel::High,
                "Rolling back an applied transaction requires explicit confirmation.",
                "Rollback rewrites the exact files recorded by a previous transaction.",
                "Inspect transaction state and approve this exact one-shot rollback.",
            ))
        }
    }
}

fn evaluate_write(
    request: &PolicyRequest,
    workspace_root: &Path,
    target: &PathTarget,
    expectation: PathExpectation,
    creation: bool,
    paths: &mut Vec<PathSafetyReport>,
) -> Result<RuleOutcome, PathSafetyError> {
    let report = inspect_path(target, expectation)?;
    let active = validated_active_worktree(request, workspace_root)?;
    if request.mode == PolicyMode::ReadOnly {
        return Ok(read_only_write_denied());
    }
    if active.as_ref() != Some(&report.root) {
        return Ok(denied(
            "write.original_requires_apply",
            RiskLevel::Critical,
            "Direct writes to the original workspace are blocked.",
            "Original changes must use a verified ApplyPatch action and one-shot approval.",
            "Create and verify a proposal in an active OpticCode worktree first.",
            false,
        ));
    }
    paths.push(report);
    if creation {
        Ok(allowed(
            "write.active_worktree_create",
            "active_opticcode_worktree",
            &[
                "validated lease",
                "controlled temporary root",
                "new path validated by the proposal contract",
                "transactional write",
            ],
            "The new file may be created inside the active disposable worktree.",
            "The original workspace remains untouched until a separate approved apply.",
        ))
    } else {
        Ok(allowed(
            "write.active_worktree",
            "active_opticcode_worktree",
            &[
                "validated lease",
                "controlled temporary root",
                "expected file hash",
                "transactional write",
            ],
            "The existing file may be changed inside the active disposable worktree.",
            "The original workspace is outside the target root and remains untouched.",
        ))
    }
}

fn evaluate_apply_patch(
    request: &PolicyRequest,
    workspace_root: &Path,
    action: &ApplyPatchAction,
    paths: &mut Vec<PathSafetyReport>,
) -> Result<RuleOutcome, PathSafetyError> {
    validate_hash_value(&action.diff_hash, "diff hash")?;
    validate_hash_value(&action.files_hash, "file-set hash")?;
    validate_identifier_value(&action.transaction_id, "transaction ID")?;
    validate_git_oid(&action.base_head, "base HEAD")?;
    validate_sorted_paths(&action.paths)?;
    validate_sorted_paths(&action.created_paths)?;
    if action
        .paths
        .len()
        .saturating_add(action.created_paths.len())
        > MAX_POLICY_PATHS
    {
        return Ok(denied(
            "action.path_limit",
            RiskLevel::High,
            "The patch contains too many paths.",
            "The structured patch exceeded the policy path bound.",
            "Split the proposal into smaller reviewed transactions.",
            true,
        ));
    }
    let root = inspect_root(&action.root)?;
    let active = validated_active_worktree(request, workspace_root)?;
    for path in &action.paths {
        paths.push(inspect_path(
            &PathTarget {
                root: root.clone(),
                path: path.clone(),
                range: None,
                expected_hash: None,
            },
            PathExpectation::File,
        )?);
    }
    for path in &action.created_paths {
        paths.push(inspect_path(
            &PathTarget {
                root: root.clone(),
                path: path.clone(),
                range: None,
                expected_hash: None,
            },
            PathExpectation::NewFile,
        )?);
    }
    if root == workspace_root {
        if request.mode != PolicyMode::ApprovedApply {
            return Ok(denied(
                "apply.mode_required",
                RiskLevel::Critical,
                "Applying to the original workspace requires approved_apply mode.",
                "The request did not enter the narrow state-bound apply mode.",
                "Verify the proposal, obtain native confirmation, then retry once.",
                true,
            ));
        }
        let Some(repository) = request.workspace.repository.as_ref() else {
            return Ok(denied(
                "apply.git_boundary_required",
                RiskLevel::Critical,
                "Applying to the original workspace requires a complete Git boundary.",
                "The runtime cannot bind approval to an observed repository HEAD without Git metadata.",
                "Rebuild the verified proposal state from the repository adapter.",
                true,
            ));
        };
        if repository.head != action.base_head {
            return Ok(denied(
                "apply.head_mismatch",
                RiskLevel::Critical,
                "Repository HEAD no longer matches the verified proposal.",
                "The apply action base commit differs from the currently observed Git boundary.",
                "Re-verify the proposal against the current HEAD.",
                true,
            ));
        }
        if request.workspace.repository_clean != Some(true) {
            return Ok(denied(
                "apply.repository_dirty",
                RiskLevel::Critical,
                "Applying to a dirty or unobserved repository is blocked.",
                "approved_apply requires an explicit clean observation from Git State Guard.",
                "Refresh Git state and resolve all pre-existing changes before approval.",
                true,
            ));
        }
        require_working_tree_digest(request)?;
        return Ok(approval_required(
            "apply.original_requires_approval",
            RiskLevel::High,
            "Applying verified changes to the original workspace requires confirmation.",
            "The approval binds request, workspace, mode, HEAD, working tree, diff, files, actions, and transaction.",
            "Review the verified diff in the IDE and confirm the native modal.",
        ));
    }
    if active.as_ref() == Some(&root) && request.mode == PolicyMode::WorktreeEdit {
        return Ok(allowed(
            "apply.active_worktree",
            "active_opticcode_worktree",
            &[
                "validated lease",
                "transactional patch",
                "bounded file set",
                "created paths validated by the proposal contract",
                "post-apply verification",
            ],
            "The patch may be applied transactionally inside the disposable worktree.",
            "The original workspace is outside the target transaction.",
        ));
    }
    Ok(outside_scope_denied())
}

fn evaluate_process(
    request: &PolicyRequest,
    workspace_root: &Path,
    action: &RunProcessAction,
    paths: &mut Vec<PathSafetyReport>,
) -> Result<RuleOutcome, PathSafetyError> {
    if request.mode != PolicyMode::WorktreeEdit {
        return Ok(read_only_write_denied());
    }
    let Some(active) = validated_active_worktree(request, workspace_root)? else {
        return Ok(outside_scope_denied());
    };
    let cwd = inspect_root(&action.cwd)?;
    if !cwd.starts_with(&active) {
        return Ok(outside_scope_denied());
    }
    if cwd != active {
        let relative = cwd.strip_prefix(&active).map_err(|_| PathSafetyError {
            rule_id: "process.cwd_outside_worktree",
            message: "process cwd is outside the active worktree".to_string(),
        })?;
        paths.push(inspect_path(
            &PathTarget {
                root: active.clone(),
                path: relative.to_path_buf(),
                range: None,
                expected_hash: None,
            },
            PathExpectation::Directory,
        )?);
    }
    if action.timeout_ms == 0 || action.timeout_ms > MAX_POLICY_TIMEOUT_MS {
        return Ok(denied(
            "process.timeout_invalid",
            RiskLevel::High,
            "The process timeout is outside policy bounds.",
            "A bounded timeout is mandatory for every child process.",
            "Use a timeout between 1 ms and one hour.",
            true,
        ));
    }
    if action.output_limit_bytes == 0 || action.output_limit_bytes > MAX_POLICY_OUTPUT_BYTES {
        return Ok(denied(
            "process.output_limit_invalid",
            RiskLevel::High,
            "The process output bound is invalid.",
            "Bounded stdout and stderr capture is mandatory.",
            "Use an output limit up to 16 MiB per stream.",
            true,
        ));
    }
    if action.arguments.len() > MAX_POLICY_ARGUMENTS
        || action.arguments.iter().map(String::len).sum::<usize>() > MAX_POLICY_ARGUMENT_BYTES
    {
        return Ok(denied(
            "process.arguments_too_large",
            RiskLevel::High,
            "The process argument list exceeds policy bounds.",
            "Arguments must remain separately represented and bounded.",
            "Reduce the argument list.",
            true,
        ));
    }
    if let Some(rule) = validate_environment_allowlist(&action.environment_allowlist) {
        return Ok(rule);
    }
    if action
        .arguments
        .iter()
        .any(|argument| unsafe_argument(argument, action.launch))
    {
        return Ok(denied(
            "process.metacharacter_denied",
            RiskLevel::Critical,
            "Shell composition and control characters are blocked.",
            "At least one argument contains a shell operator, newline, NUL, or command substitution.",
            "Pass plain, separate build-tool arguments only.",
            false,
        ));
    }
    if action.network == NetworkIntent::Undeclared {
        return Ok(denied(
            "network.undeclared",
            RiskLevel::Critical,
            "Undeclared network access is blocked.",
            "Every process must declare whether it may contact the network.",
            "Declare denied, declared, or required network intent.",
            true,
        ));
    }

    let executable_name = action
        .executable
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if is_shell_executable(&executable_name) || action.launch == ProcessLaunch::Shell {
        return Ok(denied(
            "process.shell_denied",
            RiskLevel::Critical,
            "Arbitrary shell execution is blocked.",
            "PowerShell, cmd, bash, and composed shell launch are outside the process allowlist.",
            "Use a supported build executable with separate arguments.",
            false,
        ));
    }
    let (tool, project_wrapper) = match executable_name.as_str() {
        "mvn" | "mvn.exe" | "mvn.cmd" => ("maven", false),
        "mvnw" | "mvnw.cmd" => ("maven", true),
        "gradle" | "gradle.exe" | "gradle.bat" => ("gradle", false),
        "gradlew" | "gradlew.bat" => ("gradle", true),
        "java" | "java.exe" | "javac" | "javac.exe" => {
            return Ok(denied(
                "process.java_runtime_denied",
                RiskLevel::Critical,
                "Direct Java execution is not enabled by this policy version.",
                "A generic Java or javac command can load arbitrary classes or annotation processors.",
                "Use an allowlisted Maven or Gradle verification task in the disposable worktree.",
                false,
            ));
        }
        _ => {
            return Ok(denied(
                "process.executable_unknown",
                RiskLevel::Critical,
                "The executable is not in the build allowlist.",
                "Only Maven and Gradle verification executables are supported.",
                "Use the project Maven or Gradle wrapper with bounded arguments.",
                false,
            ));
        }
    };
    if project_wrapper {
        let executable = if action.executable.is_absolute() {
            action.executable.clone()
        } else {
            cwd.join(&action.executable)
        };
        let wrapper = inspect_path(
            &PathTarget {
                root: active.clone(),
                path: executable,
                range: None,
                expected_hash: None,
            },
            PathExpectation::File,
        )?;
        let source_wrapper = inspect_path(
            &PathTarget {
                root: workspace_root.to_path_buf(),
                path: wrapper.relative_path.clone(),
                range: None,
                expected_hash: None,
            },
            PathExpectation::File,
        )?;
        if wrapper.fingerprint.content_hash != source_wrapper.fingerprint.content_hash {
            return Ok(denied(
                "process.wrapper_drift",
                RiskLevel::Critical,
                "A modified project build wrapper cannot be executed.",
                "The wrapper content in the disposable worktree differs from the observed source wrapper.",
                "Restore the wrapper and keep proposal edits outside build launchers.",
                false,
            ));
        }
        paths.push(wrapper);
        paths.push(source_wrapper);
        let command_script =
            cfg!(windows) && matches!(executable_name.as_str(), "mvnw.cmd" | "gradlew.bat");
        let expected_launch = if command_script {
            ProcessLaunch::WindowsCommandScript
        } else {
            ProcessLaunch::Direct
        };
        if action.launch != expected_launch {
            return Ok(denied(
                "process.wrapper_launch_invalid",
                RiskLevel::High,
                "The project build wrapper launch mode is invalid.",
                "Windows command scripts use the dedicated launcher; executable wrappers launch directly.",
                "Use the launch mode associated with the validated wrapper type.",
                true,
            ));
        }
    } else {
        let Some(report) = inspect_installed_executable(&action.executable)? else {
            return Ok(denied(
                "process.executable_untrusted",
                RiskLevel::Critical,
                "The installed build executable is not from a trusted absolute location.",
                "Bare PATH lookup and executables outside system installation roots can be shadowed.",
                "Use the validated project wrapper or an absolute Maven/Gradle installation path.",
                false,
            ));
        };
        paths.push(report);
        let command_script = matches!(executable_name.as_str(), "mvn.cmd" | "gradle.bat");
        let expected_launch = if cfg!(windows) && command_script {
            ProcessLaunch::WindowsCommandScript
        } else {
            ProcessLaunch::Direct
        };
        if action.launch != expected_launch {
            return Ok(denied(
                "process.installed_launch_invalid",
                RiskLevel::High,
                "The installed build executable launch mode is invalid.",
                "Native executables use direct launch; validated Windows scripts use the dedicated script launcher.",
                "Use the launch mode associated with the validated executable type.",
                true,
            ));
        }
    }
    if package_install_goal(tool, &action.arguments) {
        return Ok(denied(
            "process.package_install_denied",
            RiskLevel::Critical,
            "Install and dependency-management goals are blocked.",
            "The build invocation requests a machine or dependency state mutation.",
            "Use compile, test, verify, package, or check goals.",
            false,
        ));
    }
    if dangerous_build_option(tool, &action.arguments) {
        return Ok(denied(
            "process.option_denied",
            RiskLevel::Critical,
            "The build command contains an option outside the verification allowlist.",
            "Project, settings, init-script, extension, response-file, or tool-home overrides can escape the reviewed build boundary.",
            "Use the project defaults and ordinary verification flags only.",
            false,
        ));
    }
    if !allowed_build_goals(tool, &action.arguments) {
        return Ok(denied(
            "process.goal_unknown",
            RiskLevel::High,
            "The build goal is not in the verification allowlist.",
            "Only compile, test, verify, package, and check-style tasks are supported.",
            "Select an allowlisted verification goal.",
            true,
        ));
    }
    let offline = action
        .arguments
        .iter()
        .any(|argument| argument == "-o" || argument == "--offline");
    if offline && action.network == NetworkIntent::Denied {
        return Ok(allowed(
            "process.build_offline",
            "active_worktree_build",
            &[
                "Maven or Gradle allowlist",
                "offline",
                "bounded timeout",
                "bounded output",
                "active worktree cwd",
            ],
            "The offline build or test may run in the disposable worktree.",
            "Executable, arguments, cwd, timeout, output, and network intent passed policy.",
        ));
    }
    if action.network == NetworkIntent::Denied {
        return Ok(denied(
            "process.offline_required",
            RiskLevel::High,
            "A network-denied build must explicitly use offline mode.",
            "The process requested network denial without Maven or Gradle's offline flag.",
            "Add --offline (or -o) or declare the exact network requirement for approval.",
            true,
        ));
    }
    Ok(approval_required(
        "process.build_network_requires_approval",
        RiskLevel::Medium,
        "This Maven or Gradle verification may access the network.",
        "The invocation is bounded but is not provably offline.",
        "Confirm the exact build command and declared network access.",
    ))
}

fn evaluate_cleanup(
    request: &PolicyRequest,
    workspace_root: &Path,
    action: &CleanupWorktreeAction,
) -> Result<RuleOutcome, PathSafetyError> {
    validate_worktree_run_id(&action.run_id)?;
    if request.mode == PolicyMode::ReadOnly {
        return Ok(read_only_write_denied());
    }
    let repository = inspect_root(&action.repository_root)?;
    if repository != workspace_root {
        return Ok(outside_scope_denied());
    }
    let active = validated_active_worktree(request, workspace_root)?;
    if active.as_ref() == Some(&inspect_root(&action.worktree_root)?)
        && request.mode == PolicyMode::WorktreeEdit
        && request
            .workspace
            .active_worktree
            .as_ref()
            .is_some_and(|worktree| worktree.run_id == action.run_id)
    {
        return Ok(allowed(
            "worktree.cleanup_active",
            "active_opticcode_worktree",
            &[
                "validated lease",
                "targeted removal",
                "registration recheck",
            ],
            "The active disposable worktree may be cleaned up.",
            "Cleanup is restricted to the exact leased worktree and never uses global prune.",
        ));
    }
    Ok(approval_required(
        "worktree.recovery_requires_approval",
        RiskLevel::High,
        "Recovering an abandoned worktree requires confirmation.",
        "The target is not the current active worktree for this request.",
        "Inspect the lease and approve targeted recovery.",
    ))
}

fn validated_active_worktree(
    request: &PolicyRequest,
    workspace_root: &Path,
) -> Result<Option<PathBuf>, PathSafetyError> {
    let active = validate_active_descriptor(
        request.workspace.active_worktree.as_ref(),
        workspace_root,
        &request.workspace.workspace_id,
        &request.request_id,
    )?;
    if let Some(descriptor) = request.workspace.active_worktree.as_ref() {
        let Some(repository) = request.workspace.repository.as_ref() else {
            return Err(PathSafetyError {
                rule_id: "worktree.source_state_missing",
                message: "active worktree requires an observed source repository boundary"
                    .to_string(),
            });
        };
        if repository.head != descriptor.base_head {
            return Err(PathSafetyError {
                rule_id: "worktree.source_head_drift",
                message: "source HEAD changed after the worktree lease was created".to_string(),
            });
        }
        if request.workspace.repository_clean != Some(true) {
            return Err(PathSafetyError {
                rule_id: "worktree.source_dirty",
                message: "source repository is dirty or its cleanliness is unobserved".to_string(),
            });
        }
        require_working_tree_digest(request)?;
    }
    Ok(active)
}

fn validate_active_descriptor(
    descriptor: Option<&ActiveWorktree>,
    workspace_root: &Path,
    workspace_id: &str,
    request_id: &str,
) -> Result<Option<PathBuf>, PathSafetyError> {
    let Some(descriptor) = descriptor else {
        return Ok(None);
    };
    validate_worktree_run_id(&descriptor.run_id)?;
    validate_identifier_value(
        &descriptor.owner_workspace_id,
        "worktree owner workspace ID",
    )?;
    validate_identifier_value(&descriptor.owner_request_id, "worktree owner request ID")?;
    validate_git_oid(&descriptor.base_head, "worktree base HEAD")?;
    if descriptor.owner_workspace_id != workspace_id || descriptor.owner_request_id != request_id {
        return Err(PathSafetyError {
            rule_id: "worktree.owner_mismatch",
            message: "active worktree does not belong to this workspace and request".to_string(),
        });
    }
    let source = inspect_root(&descriptor.source_root)?;
    if source != workspace_root {
        return Err(PathSafetyError {
            rule_id: "worktree.source_mismatch",
            message: "active worktree source does not match the workspace".to_string(),
        });
    }
    let root = inspect_root(&descriptor.root)?;
    let storage = controlled_worktree_storage()?;
    let expected = storage
        .join(WORKTREE_RUNS_DIRECTORY)
        .join(&descriptor.run_id);
    let expected = fs::canonicalize(&expected).map_err(|_| PathSafetyError {
        rule_id: "worktree.storage_mismatch",
        message: "active worktree is not in controlled temporary storage".to_string(),
    })?;
    if root != expected {
        return Err(PathSafetyError {
            rule_id: "worktree.storage_mismatch",
            message: "active worktree path does not match its controlled run ID".to_string(),
        });
    }
    validate_worktree_lease(
        &storage,
        descriptor,
        &root,
        workspace_root,
        workspace_id,
        request_id,
    )?;
    validate_git_file(&root, descriptor)?;
    Ok(Some(root))
}

fn validate_worktree_lease(
    storage: &Path,
    descriptor: &ActiveWorktree,
    root: &Path,
    workspace_root: &Path,
    workspace_id: &str,
    request_id: &str,
) -> Result<(), PathSafetyError> {
    let lease_path = storage
        .join(WORKTREE_LEASES_DIRECTORY)
        .join(format!("{}.json", descriptor.run_id));
    let metadata = fs::symlink_metadata(&lease_path).map_err(|_| PathSafetyError {
        rule_id: "worktree.lease_missing",
        message: "active worktree has no runtime lease".to_string(),
    })?;
    if !metadata.is_file()
        || metadata_is_link_or_reparse(&metadata)
        || metadata.len() > MAX_LEASE_BYTES
    {
        return Err(PathSafetyError {
            rule_id: "worktree.lease_invalid",
            message: "active worktree lease is not a bounded regular file".to_string(),
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&lease_path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|_| PathSafetyError {
            rule_id: "worktree.lease_invalid",
            message: "active worktree lease cannot be read".to_string(),
        })?;
    let lease: WorktreeLeaseWire = serde_json::from_slice(&bytes).map_err(|_| PathSafetyError {
        rule_id: "worktree.lease_invalid",
        message: "active worktree lease schema is invalid".to_string(),
    })?;
    if lease.schema_version != 1
        || lease.run_id != descriptor.run_id
        || fs::canonicalize(&lease.source_git_root).ok().as_deref() != Some(workspace_root)
        || fs::canonicalize(&lease.worktree_path).ok().as_deref() != Some(root)
        || lease.source_commit != descriptor.base_head
        || lease.owner_workspace_id != workspace_id
        || lease.owner_request_id != request_id
    {
        return Err(PathSafetyError {
            rule_id: "worktree.lease_mismatch",
            message: "active worktree does not match its runtime lease".to_string(),
        });
    }
    Ok(())
}

fn validate_git_file(root: &Path, descriptor: &ActiveWorktree) -> Result<(), PathSafetyError> {
    let git_file = root.join(".git");
    let metadata = fs::symlink_metadata(&git_file).map_err(|_| PathSafetyError {
        rule_id: "git.worktree_file_missing",
        message: "active worktree is missing its .git indirection file".to_string(),
    })?;
    if !metadata.is_file() || metadata_is_link_or_reparse(&metadata) || metadata.len() > 8 * 1024 {
        return Err(PathSafetyError {
            rule_id: "git.worktree_file_invalid",
            message: "worktree .git indirection is unsafe".to_string(),
        });
    }
    let content = fs::read_to_string(&git_file).map_err(|_| PathSafetyError {
        rule_id: "git.worktree_file_invalid",
        message: "worktree .git indirection cannot be read".to_string(),
    })?;
    let path = content
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .map(PathBuf::from)
        .ok_or_else(|| PathSafetyError {
            rule_id: "git.worktree_file_invalid",
            message: "worktree .git indirection has an invalid format".to_string(),
        })?;
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let git_dir = fs::canonicalize(path).map_err(|_| PathSafetyError {
        rule_id: "git.worktree_gitdir_invalid",
        message: "worktree gitdir cannot be resolved".to_string(),
    })?;
    let expected_git_dir = fs::canonicalize(&descriptor.git_dir).map_err(|_| PathSafetyError {
        rule_id: "git.worktree_gitdir_invalid",
        message: "declared worktree gitdir cannot be resolved".to_string(),
    })?;
    let common_dir = fs::canonicalize(&descriptor.common_dir).map_err(|_| PathSafetyError {
        rule_id: "git.commondir_invalid",
        message: "declared Git common directory cannot be resolved".to_string(),
    })?;
    if git_dir != expected_git_dir || git_dir == common_dir || !git_dir.starts_with(&common_dir) {
        return Err(PathSafetyError {
            rule_id: "git.boundary_mismatch",
            message: "worktree gitdir and common Git directory boundaries do not match".to_string(),
        });
    }
    let commondir_file = git_dir.join("commondir");
    let metadata = fs::symlink_metadata(&commondir_file).map_err(|_| PathSafetyError {
        rule_id: "git.commondir_invalid",
        message: "worktree gitdir is missing its commondir file".to_string(),
    })?;
    if !metadata.is_file() || metadata_is_link_or_reparse(&metadata) || metadata.len() > 8 * 1024 {
        return Err(PathSafetyError {
            rule_id: "git.commondir_invalid",
            message: "worktree commondir indirection is unsafe".to_string(),
        });
    }
    let commondir_value = fs::read_to_string(&commondir_file).map_err(|_| PathSafetyError {
        rule_id: "git.commondir_invalid",
        message: "worktree commondir indirection cannot be read".to_string(),
    })?;
    let commondir_path = PathBuf::from(commondir_value.trim());
    let commondir_path = if commondir_path.is_absolute() {
        commondir_path
    } else {
        git_dir.join(commondir_path)
    };
    if fs::canonicalize(commondir_path).ok().as_deref() != Some(common_dir.as_path()) {
        return Err(PathSafetyError {
            rule_id: "git.commondir_invalid",
            message: "worktree commondir file differs from the declared Git boundary".to_string(),
        });
    }
    Ok(())
}

fn validate_worktree_destination(destination: &Path, run_id: &str) -> Result<(), PathSafetyError> {
    validate_worktree_run_id(run_id)?;
    let storage = controlled_worktree_storage()?;
    let runs =
        fs::canonicalize(storage.join(WORKTREE_RUNS_DIRECTORY)).map_err(|_| PathSafetyError {
            rule_id: "worktree.storage_unavailable",
            message: "controlled worktree runs directory is unavailable".to_string(),
        })?;
    let destination_parent = destination
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok());
    if destination_parent.as_deref() != Some(runs.as_path())
        || destination.file_name().and_then(|value| value.to_str()) != Some(run_id)
    {
        return Err(PathSafetyError {
            rule_id: "worktree.destination_denied",
            message: "worktree destination is outside the controlled run directory".to_string(),
        });
    }
    if destination.exists() {
        return Err(PathSafetyError {
            rule_id: "worktree.destination_exists",
            message: "worktree destination already exists".to_string(),
        });
    }
    Ok(())
}

fn controlled_worktree_storage() -> Result<PathBuf, PathSafetyError> {
    inspect_root(&std::env::temp_dir().join(WORKTREE_STORAGE_DIRECTORY))
}

fn inspect_installed_executable(
    executable: &Path,
) -> Result<Option<PathSafetyReport>, PathSafetyError> {
    if !executable.is_absolute() {
        return Ok(None);
    }
    let canonical = match fs::canonicalize(executable) {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    let mut roots = Vec::new();
    #[cfg(windows)]
    {
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(variable) {
                roots.push(PathBuf::from(root));
            }
        }
    }
    #[cfg(not(windows))]
    {
        roots.extend([
            PathBuf::from("/usr/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt"),
        ]);
    }
    for root in roots {
        let Ok(root) = inspect_root(&root) else {
            continue;
        };
        if canonical.starts_with(&root) {
            return inspect_path(
                &PathTarget {
                    root,
                    path: canonical,
                    range: None,
                    expected_hash: None,
                },
                PathExpectation::File,
            )
            .map(Some);
        }
    }
    Ok(None)
}

fn require_read_scope(
    request: &PolicyRequest,
    workspace_root: &Path,
    action_root: &Path,
) -> Result<(), PathSafetyError> {
    if action_root == workspace_root {
        return Ok(());
    }
    if validated_active_worktree(request, workspace_root)?.as_ref()
        == Some(&action_root.to_path_buf())
    {
        return Ok(());
    }
    Err(PathSafetyError {
        rule_id: "path.outside_workspace",
        message: "read root is outside the workspace and active worktree".to_string(),
    })
}

fn require_workspace_or_active_root(
    request: &PolicyRequest,
    workspace_root: &Path,
    action_root: &Path,
) -> Result<(), PathSafetyError> {
    require_read_scope(request, workspace_root, action_root)
}

fn inspect_path_list(
    root: &Path,
    input: &[PathBuf],
    paths: &mut Vec<PathSafetyReport>,
) -> Result<(), PathSafetyError> {
    if input.len() > MAX_POLICY_PATHS {
        return Err(PathSafetyError {
            rule_id: "action.path_limit",
            message: "action contains too many paths".to_string(),
        });
    }
    validate_sorted_paths(input)?;
    for path in input {
        paths.push(inspect_path(
            &PathTarget {
                root: root.to_path_buf(),
                path: path.clone(),
                range: None,
                expected_hash: None,
            },
            PathExpectation::ExistingEntry,
        )?);
    }
    Ok(())
}

fn validate_transaction_action(
    action: &crate::model::TransactionAction,
    workspace_root: &Path,
) -> Result<(), PathSafetyError> {
    if inspect_root(&action.workspace_root)? != workspace_root {
        return Err(PathSafetyError {
            rule_id: "transaction.workspace_mismatch",
            message: "transaction belongs to a different workspace".to_string(),
        });
    }
    validate_identifier_value(&action.transaction_id, "transaction ID")?;
    validate_hash_value(&action.expected_state_hash, "transaction state hash")
}

fn validate_request(request: &PolicyRequest) -> std::result::Result<(), PolicyError> {
    if request.schema_version != POLICY_SCHEMA_VERSION || request.protocol != POLICY_PROTOCOL_ID {
        return Err(PolicyError::InvalidRequest(format!(
            "expected {POLICY_PROTOCOL_ID} schema {POLICY_SCHEMA_VERSION}"
        )));
    }
    validate_request_identifier(
        &request.request_id,
        MAX_POLICY_REQUEST_ID_BYTES,
        "request ID",
    )?;
    validate_request_identifier(&request.action_id, MAX_POLICY_IDENTIFIER_BYTES, "action ID")?;
    validate_request_identifier(
        &request.workspace.workspace_id,
        MAX_POLICY_IDENTIFIER_BYTES,
        "workspace ID",
    )?;
    validate_request_identifier(&request.profile, 96, "profile")?;
    validate_request_identifier(&request.client.name, 96, "client name")?;
    validate_request_identifier(&request.client.version, 96, "client version")?;
    if request.origin == ActionOrigin::Unknown {
        return Err(PolicyError::InvalidRequest(
            "unknown action origin is not accepted".to_string(),
        ));
    }
    if request.approval_id.as_ref().is_some_and(|value| {
        value.is_empty()
            || value.len() > MAX_POLICY_IDENTIFIER_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }) {
        return Err(PolicyError::InvalidRequest(
            "approval ID is malformed".to_string(),
        ));
    }
    if let Some(digest) = request.workspace.working_tree_digest.as_deref() {
        validate_hash_value(digest, "working-tree digest").map_err(invalid_path_request)?;
    }
    validate_repository_boundary(
        request.workspace.repository.as_ref(),
        &request.workspace.root,
    )
    .map_err(invalid_path_request)?;
    Ok(())
}

fn validate_repository_boundary(
    boundary: Option<&GitRepositoryBoundary>,
    workspace: &Path,
) -> Result<(), PathSafetyError> {
    let Some(boundary) = boundary else {
        return Ok(());
    };
    let workspace = inspect_root(workspace)?;
    let worktree = inspect_root(&boundary.worktree_root)?;
    if workspace != worktree {
        return Err(PathSafetyError {
            rule_id: "git.workspace_mismatch",
            message: "declared repository root differs from the workspace root".to_string(),
        });
    }
    let git_dir = inspect_root(&boundary.git_dir)?;
    let common_dir = inspect_root(&boundary.common_dir)?;
    let object_dir = inspect_root(&boundary.object_dir)?;
    if boundary.main_worktree && git_dir != common_dir {
        return Err(PathSafetyError {
            rule_id: "git.boundary_mismatch",
            message: "main worktree gitdir must match its common directory".to_string(),
        });
    }
    if boundary.main_worktree {
        let expected_git_dir = workspace.join(".git");
        if fs::canonicalize(expected_git_dir).ok().as_deref() != Some(git_dir.as_path()) {
            return Err(PathSafetyError {
                rule_id: "git.boundary_mismatch",
                message: "main Git directory is not the workspace .git directory".to_string(),
            });
        }
    }
    if !boundary.main_worktree && (git_dir == common_dir || !git_dir.starts_with(&common_dir)) {
        return Err(PathSafetyError {
            rule_id: "git.boundary_mismatch",
            message: "linked worktree gitdir is outside the declared common directory".to_string(),
        });
    }
    if object_dir != common_dir.join("objects") {
        return Err(PathSafetyError {
            rule_id: "git.object_boundary_mismatch",
            message: "Git object directory is not the exact common objects directory".to_string(),
        });
    }
    if !matches!(boundary.head.len(), 40 | 64)
        || !boundary.head.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PathSafetyError {
            rule_id: "git.head_invalid",
            message: "repository HEAD is not a full hexadecimal object ID".to_string(),
        });
    }
    let index_metadata = fs::symlink_metadata(&boundary.index).map_err(|_| PathSafetyError {
        rule_id: "git.index_missing",
        message: "Git index boundary does not exist or cannot be inspected".to_string(),
    })?;
    if !index_metadata.is_file()
        || metadata_is_link_or_reparse(&index_metadata)
        || boundary.index.file_name().and_then(|value| value.to_str()) != Some("index")
        || boundary
            .index
            .parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .as_deref()
            != Some(git_dir.as_path())
    {
        return Err(PathSafetyError {
            rule_id: "git.index_missing",
            message: "Git index boundary is empty or outside the worktree gitdir".to_string(),
        });
    }
    Ok(())
}

fn approval_binding(
    request: &PolicyRequest,
    paths: &[PathSafetyReport],
) -> std::result::Result<ApprovalBinding, PolicyError> {
    let action_hash = action_hash(&request.action)?;
    let workspace_root = inspect_root(&request.workspace.root).map_err(invalid_path_request)?;
    let working_tree_digest = request
        .workspace
        .working_tree_digest
        .as_deref()
        .ok_or_else(|| {
            PolicyError::InvalidRequest(
                "approval requires an observed working-tree digest".to_string(),
            )
        })?;
    validate_hash_value(working_tree_digest, "working-tree digest")
        .map_err(invalid_path_request)?;
    let files = if paths.is_empty() {
        vec![ApprovalFileBinding {
            path_hash: hash_text(&normalized_absolute(&workspace_root)),
            expected_hash: hash_text("no_file_binding"),
        }]
    } else {
        paths
            .iter()
            .map(|path| ApprovalFileBinding {
                path_hash: hash_text(&format!(
                    "{}:{}",
                    normalized_absolute(&path.root),
                    path.relative_path.to_string_lossy().replace('\\', "/")
                )),
                expected_hash: path
                    .fingerprint
                    .content_hash
                    .clone()
                    .unwrap_or_else(|| path.fingerprint.metadata_hash.clone()),
            })
            .collect()
    };
    let (base_head, diff_hash, files_hash, transaction_id) = match &request.action {
        PolicyAction::ApplyPatch(action) => (
            action.base_head.clone(),
            action.diff_hash.clone(),
            action.files_hash.clone(),
            action.transaction_id.clone(),
        ),
        PolicyAction::GitCommit(action) => (
            request.workspace.repository.as_ref().map_or_else(
                || "no-head".to_string(),
                |repository| repository.head.clone(),
            ),
            action.tree_hash.clone(),
            hash_file_bindings(&files)?,
            format!("commit-{}", &action.tree_hash[..16]),
        ),
        PolicyAction::RecoverTransaction(action) | PolicyAction::RollbackTransaction(action) => (
            request.workspace.repository.as_ref().map_or_else(
                || "no-head".to_string(),
                |repository| repository.head.clone(),
            ),
            action.expected_state_hash.clone(),
            hash_file_bindings(&files)?,
            action.transaction_id.clone(),
        ),
        _ => (
            request.workspace.repository.as_ref().map_or_else(
                || "no-head".to_string(),
                |repository| repository.head.clone(),
            ),
            action_hash.clone(),
            hash_file_bindings(&files)?,
            format!("action-{}", &action_hash[..16]),
        ),
    };
    ApprovalBinding {
        request_id: request.request_id.clone(),
        workspace_id: request.workspace.workspace_id.clone(),
        workspace_root_hash: hash_text(&normalized_absolute(&workspace_root)),
        mode: request.mode,
        base_head,
        working_tree_digest: working_tree_digest.to_string(),
        diff_hash,
        files_hash,
        files,
        action_hashes: vec![action_hash],
        transaction_id,
    }
    .normalized()
    .map_err(|error| PolicyError::InvalidRequest(error.to_string()))
}

fn apply_approval_failure(report: &mut PolicyReport, error: ApprovalError) {
    let (rule_id, retriable) = match error {
        ApprovalError::Expired => ("approval.expired", true),
        ApprovalError::Reused => ("approval.reused", false),
        ApprovalError::WrongRequest => ("approval.request_drift", false),
        ApprovalError::WrongWorkspace => ("approval.workspace_drift", false),
        ApprovalError::WrongMode => ("approval.mode_drift", false),
        ApprovalError::HeadChanged => ("approval.head_drift", true),
        ApprovalError::WorkingTreeChanged => ("approval.working_tree_drift", true),
        ApprovalError::DiffChanged => ("approval.diff_drift", true),
        ApprovalError::FilesChanged => ("approval.files_drift", true),
        ApprovalError::ActionsChanged => ("approval.actions_drift", true),
        ApprovalError::TransactionChanged => ("approval.transaction_drift", true),
        ApprovalError::Missing => ("approval.missing", true),
        ApprovalError::InvalidRecord => ("approval.invalid", false),
    };
    report.decision = PolicyDecision::Deny {
        rule_id: rule_id.to_string(),
        reason: error.to_string(),
        risk: RiskLevel::High,
    };
    report.user_reason = "The approval is no longer valid for this action.".to_string();
    report.technical_reason = bounded(&error.to_string(), MAX_TECHNICAL_REASON_BYTES);
    report.recommended_action =
        "Re-verify the proposal and request a new native confirmation.".to_string();
    report.retriable = retriable;
}

fn action_hash(action: &PolicyAction) -> std::result::Result<String, PolicyError> {
    let bytes = serde_json::to_vec(action)
        .map_err(|error| PolicyError::InvalidRequest(error.to_string()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn security_context_hash(request: &PolicyRequest) -> std::result::Result<String, PolicyError> {
    let bytes = serde_json::to_vec(request)
        .map_err(|error| PolicyError::InvalidRequest(error.to_string()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn hash_file_bindings(files: &[ApprovalFileBinding]) -> std::result::Result<String, PolicyError> {
    let bytes = serde_json::to_vec(files)
        .map_err(|error| PolicyError::InvalidRequest(error.to_string()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn allowed(
    rule_id: &str,
    scope: &str,
    conditions: &[&str],
    user_reason: &str,
    technical_reason: &str,
) -> RuleOutcome {
    (
        PolicyDecision::Allow {
            rule_id: rule_id.to_string(),
            risk: RiskLevel::Low,
            scope: scope.to_string(),
            conditions: conditions
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        },
        user_reason.to_string(),
        technical_reason.to_string(),
        "Proceed through the structured executor and revalidate before and after.".to_string(),
        false,
    )
}

fn approval_required(
    rule_id: &str,
    risk: RiskLevel,
    user_reason: &str,
    technical_reason: &str,
    recommended: &str,
) -> RuleOutcome {
    (
        PolicyDecision::RequireApproval {
            rule_id: rule_id.to_string(),
            reason: user_reason.to_string(),
            risk,
            summary: technical_reason.to_string(),
        },
        user_reason.to_string(),
        technical_reason.to_string(),
        recommended.to_string(),
        true,
    )
}

fn denied(
    rule_id: &str,
    risk: RiskLevel,
    user_reason: &str,
    technical_reason: &str,
    recommended: &str,
    retriable: bool,
) -> RuleOutcome {
    (
        PolicyDecision::Deny {
            rule_id: rule_id.to_string(),
            reason: user_reason.to_string(),
            risk,
        },
        user_reason.to_string(),
        technical_reason.to_string(),
        recommended.to_string(),
        retriable,
    )
}

fn read_only_write_denied() -> RuleOutcome {
    denied(
        "mode.read_only_write_denied",
        RiskLevel::High,
        "The current read_only mode cannot write or mutate state.",
        "No approval can widen read_only mode implicitly.",
        "Start an explicit worktree proposal or approved apply flow.",
        true,
    )
}

fn outside_scope_denied() -> RuleOutcome {
    denied(
        "path.outside_authorized_root",
        RiskLevel::Critical,
        "The action target is outside its authorized root.",
        "The target is neither the canonical workspace nor its active leased worktree.",
        "Use a path inside the current workspace or active proposal worktree.",
        false,
    )
}

fn validate_sorted_paths(paths: &[PathBuf]) -> Result<(), PathSafetyError> {
    let normalized = paths
        .iter()
        .map(|path| {
            if path.is_absolute()
                || path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(PathSafetyError {
                    rule_id: "path.invalid_relative",
                    message: "file lists must contain normal relative paths".to_string(),
                });
            }
            Ok(path.to_string_lossy().replace('\\', "/"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if normalized.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(PathSafetyError {
            rule_id: "action.path_order",
            message: "file lists must be unique and sorted deterministically".to_string(),
        });
    }
    Ok(())
}

fn validate_hash_value(value: &str, label: &str) -> Result<(), PathSafetyError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PathSafetyError {
            rule_id: "action.hash_invalid",
            message: format!("{label} is not a 64-character hexadecimal digest"),
        });
    }
    Ok(())
}

fn validate_git_oid(value: &str, label: &str) -> Result<(), PathSafetyError> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PathSafetyError {
            rule_id: "git.oid_invalid",
            message: format!("{label} is not a full hexadecimal Git object ID"),
        });
    }
    Ok(())
}

fn require_working_tree_digest(request: &PolicyRequest) -> Result<(), PathSafetyError> {
    let Some(digest) = request.workspace.working_tree_digest.as_deref() else {
        return Err(PathSafetyError {
            rule_id: "git.working_tree_unobserved",
            message: "working-tree digest is required for this mutating action".to_string(),
        });
    };
    validate_hash_value(digest, "working-tree digest")
}

fn validate_identifier_value(value: &str, label: &str) -> Result<(), PathSafetyError> {
    if value.is_empty()
        || value.len() > MAX_POLICY_IDENTIFIER_BYTES
        || value.contains(['\r', '\n', '\0'])
    {
        return Err(PathSafetyError {
            rule_id: "action.identifier_invalid",
            message: format!("{label} is empty, too long, or contains control characters"),
        });
    }
    Ok(())
}

fn validate_worktree_run_id(value: &str) -> Result<(), PathSafetyError> {
    if !(1..=96).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(PathSafetyError {
            rule_id: "worktree.run_id",
            message: "worktree run ID must contain only ASCII letters, digits, '-' or '_'"
                .to_string(),
        });
    }
    Ok(())
}

fn validate_request_identifier(
    value: &str,
    max: usize,
    label: &str,
) -> std::result::Result<(), PolicyError> {
    if value.is_empty()
        || value.len() > max
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(PolicyError::InvalidRequest(format!(
            "{label} contains unsupported characters or exceeds its bound"
        )));
    }
    Ok(())
}

fn validate_environment_allowlist(environment: &[String]) -> Option<RuleOutcome> {
    if environment.len() > MAX_POLICY_ENVIRONMENT_KEYS {
        return Some(denied(
            "process.environment_limit",
            RiskLevel::High,
            "The process environment allowlist is too large.",
            "Only a bounded set of inherited environment names may reach a child process.",
            "Reduce the environment allowlist.",
            true,
        ));
    }
    if environment.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Some(denied(
            "process.environment_order",
            RiskLevel::High,
            "The process environment allowlist must be unique and sorted.",
            "Deterministic ordering prevents ambiguous or duplicated inherited variables.",
            "Sort and deduplicate the environment names.",
            true,
        ));
    }
    const ALLOWED: &[&str] = &[
        "CI",
        "GRADLE_USER_HOME",
        "JAVA_HOME",
        "MAVEN_USER_HOME",
        "NO_COLOR",
    ];
    if environment.iter().any(|name| {
        name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            || !ALLOWED.contains(&name.as_str())
    }) {
        return Some(denied(
            "process.environment_denied",
            RiskLevel::Critical,
            "The process requested an environment variable outside the allowlist.",
            "Secrets, shell configuration, JVM option injection, and arbitrary variables are not inherited.",
            "Use only the documented non-secret build environment names.",
            false,
        ));
    }
    None
}

fn unsafe_argument(argument: &str, launch: ProcessLaunch) -> bool {
    if argument.is_empty()
        || argument.contains(['\r', '\n', '\0'])
        || argument.contains(char::from(96))
        || argument.contains("$(")
    {
        return true;
    }
    const OPERATORS: &[&str] = &["&", "&&", "|", "||", ";", ">", ">>", "<", "2>"];
    let trimmed = argument.trim();
    if OPERATORS.contains(&trimmed)
        || argument
            .split_whitespace()
            .any(|part| OPERATORS.contains(&part))
    {
        return true;
    }
    launch == ProcessLaunch::WindowsCommandScript
        && argument
            .chars()
            .any(|character| matches!(character, '&' | '|' | '<' | '>' | '^' | '%' | '!'))
}

fn is_shell_executable(name: &str) -> bool {
    matches!(
        name,
        "cmd"
            | "cmd.exe"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
            | "pwsh.exe"
            | "bash"
            | "bash.exe"
            | "sh"
            | "sh.exe"
            | "wsl"
            | "wsl.exe"
    )
}

fn package_install_goal(tool: &str, arguments: &[String]) -> bool {
    arguments.iter().any(|argument| {
        let lower = argument.to_ascii_lowercase();
        (tool == "maven"
            && (lower == "install"
                || lower == "deploy"
                || lower.contains("dependency:get")
                || lower.contains("wrapper:")))
            || (tool == "gradle"
                && (lower == "wrapper"
                    || lower.contains("publish")
                    || lower.contains("dependencyupdates")))
    })
}

fn dangerous_build_option(tool: &str, arguments: &[String]) -> bool {
    arguments.iter().any(|argument| {
        let lower = argument.to_ascii_lowercase();
        if lower.starts_with('@') {
            return true;
        }
        if tool == "maven" {
            matches!(
                lower.as_str(),
                "-f" | "--file"
                    | "-s"
                    | "--settings"
                    | "-gs"
                    | "--global-settings"
                    | "-t"
                    | "--toolchains"
            ) || lower.starts_with("-dmaven.ext.class.path=")
                || lower.starts_with("-dmaven.home=")
                || lower.starts_with("-dmaven.multimoduleprojectdirectory=")
        } else {
            argument == "-I"
                || matches!(
                    lower.as_str(),
                    "-b" | "--build-file"
                        | "-c"
                        | "--settings-file"
                        | "-p"
                        | "--project-dir"
                        | "-g"
                        | "--gradle-user-home"
                        | "--init-script"
                        | "--include-build"
                )
                || lower.starts_with("--build-file=")
                || lower.starts_with("--settings-file=")
                || lower.starts_with("--project-dir=")
                || lower.starts_with("--gradle-user-home=")
                || lower.starts_with("--init-script=")
                || lower.starts_with("--include-build=")
        }
    })
}

fn allowed_build_goals(tool: &str, arguments: &[String]) -> bool {
    let goals = arguments
        .iter()
        .filter(|argument| !argument.starts_with('-'))
        .map(|argument| argument.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    if goals.is_empty() {
        return false;
    }
    goals.iter().all(|goal| {
        if tool == "maven" {
            matches!(
                goal.as_str(),
                "clean" | "compile" | "test" | "verify" | "package" | "checkstyle:check"
            )
        } else {
            matches!(
                goal.as_str(),
                "clean" | "compilejava" | "test" | "check" | "build" | "assemble"
            ) || goal.ends_with(":test")
                || goal.ends_with(":check")
                || goal.ends_with(":build")
        }
    })
}

fn invalid_path_request(error: PathSafetyError) -> PolicyError {
    PolicyError::InvalidRequest(error.to_string())
}

fn hash_text(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

fn normalized_absolute(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn bounded(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max.saturating_sub(16).min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &value[..end])
}

fn duration_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn transaction_id(action: &PolicyAction) -> Option<&str> {
    match action {
        PolicyAction::ApplyPatch(action) => Some(&action.transaction_id),
        PolicyAction::RecoverTransaction(action) | PolicyAction::RollbackTransaction(action) => {
            Some(&action.transaction_id)
        }
        _ => None,
    }
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorktreeLeaseWire {
    schema_version: u32,
    run_id: String,
    owner_workspace_id: String,
    owner_request_id: String,
    #[allow(dead_code)]
    process_id: u32,
    #[allow(dead_code)]
    created_unix_ms: u64,
    source_git_root: PathBuf,
    #[allow(dead_code)]
    source_project: PathBuf,
    source_commit: String,
    worktree_path: PathBuf,
}
