use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use opticcode_policy::{
    ActionOrigin, ActiveWorktree, GitRepositoryBoundary, PolicyAction, PolicyClient, PolicyEngine,
    PolicyMode, PolicyReport, PolicyRequest, PolicyWorkspace,
};
use opticcode_tools::git_state::capture_git_state;
use opticcode_tools::process_runner::{
    run_process_with_cancellation, CancellationToken, ProcessRequest,
};
use opticcode_tools::worktree::DisposableWorktreeContext;

use crate::{canonical_root_hash, working_tree_digest, EditRuntimeOptions, PolicyDecisionRecord};

#[derive(Debug, Clone)]
pub(crate) struct ObservedRepository {
    pub root: PathBuf,
    pub root_hash: String,
    pub boundary: GitRepositoryBoundary,
    pub working_tree_digest: String,
    pub clean: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditWorkspaceObservation {
    pub root: PathBuf,
    pub root_hash: String,
    pub base_head: String,
    pub working_tree_digest: String,
    pub clean: bool,
}

pub fn inspect_edit_workspace(
    options: &EditRuntimeOptions,
    cancellation: &CancellationToken,
) -> Result<EditWorkspaceObservation> {
    let observed = observe_repository(options, cancellation)?;
    Ok(EditWorkspaceObservation {
        root: observed.root,
        root_hash: observed.root_hash,
        base_head: observed.boundary.head,
        working_tree_digest: observed.working_tree_digest,
        clean: observed.clean,
    })
}

pub(crate) struct PolicySession<'a> {
    engine: &'a PolicyEngine,
    options: &'a EditRuntimeOptions,
    observed: &'a ObservedRepository,
}

impl<'a> PolicySession<'a> {
    pub(crate) fn new(
        engine: &'a PolicyEngine,
        options: &'a EditRuntimeOptions,
        observed: &'a ObservedRepository,
    ) -> Self {
        Self {
            engine,
            options,
            observed,
        }
    }

    pub(crate) fn authorize(
        &self,
        mode: PolicyMode,
        action: PolicyAction,
        active: Option<&DisposableWorktreeContext>,
        approval_id: Option<String>,
        stage: &str,
        records: &mut Vec<PolicyDecisionRecord>,
    ) -> Result<()> {
        let request = self.request(mode, action, active, approval_id, stage, records.len());
        let preflight = self
            .engine
            .check(&request)
            .with_context(|| format!("policy check failed for {stage}"))?;
        records.push(policy_record(stage, &preflight.report));
        if !preflight.report.allowed() {
            bail!(
                "policy denied {stage}: {}: {}",
                preflight.report.decision.rule_id(),
                preflight.report.user_reason
            );
        }
        preflight
            .revalidate()
            .with_context(|| format!("policy inputs drifted before {stage}"))?;
        Ok(())
    }

    pub(crate) fn request(
        &self,
        mode: PolicyMode,
        action: PolicyAction,
        active: Option<&DisposableWorktreeContext>,
        approval_id: Option<String>,
        stage: &str,
        sequence: usize,
    ) -> PolicyRequest {
        let active_worktree = active.map(|context| ActiveWorktree {
            run_id: context.run_id.clone(),
            owner_workspace_id: context.owner.workspace_id.clone(),
            owner_request_id: context.owner.request_id.clone(),
            root: context.worktree_root.clone(),
            source_root: context.source_root.clone(),
            base_head: context.base_head.clone(),
            git_dir: context.git_dir.clone(),
            common_dir: context.common_dir.clone(),
        });
        PolicyRequest {
            schema_version: opticcode_policy::POLICY_SCHEMA_VERSION,
            protocol: opticcode_policy::POLICY_PROTOCOL_ID.to_string(),
            request_id: self.options.request_id.clone(),
            action_id: format!("{}-{stage}-{sequence}", self.options.request_id),
            origin: ActionOrigin::Chat,
            profile: self.options.profile.clone(),
            client: PolicyClient {
                name: self.options.client_name.clone(),
                version: self.options.client_version.clone(),
            },
            mode,
            workspace: PolicyWorkspace {
                workspace_id: self.options.workspace_id.clone(),
                root: self.observed.root.clone(),
                repository: Some(self.observed.boundary.clone()),
                active_worktree,
                working_tree_digest: Some(self.observed.working_tree_digest.clone()),
                repository_clean: Some(self.observed.clean),
            },
            action,
            approval_id,
        }
    }
}

pub(crate) fn policy_record(stage: &str, report: &PolicyReport) -> PolicyDecisionRecord {
    PolicyDecisionRecord {
        stage: stage.to_string(),
        action_kind: report.action_kind.clone(),
        decision: report.decision.kind().to_string(),
        rule_id: report.decision.rule_id().to_string(),
        action_hash: report.action_hash.clone(),
        audit_event_id: report.audit_event_id.clone(),
    }
}

pub(crate) fn observe_repository(
    options: &EditRuntimeOptions,
    cancellation: &CancellationToken,
) -> Result<ObservedRepository> {
    let requested = fs::canonicalize(&options.workspace_root).with_context(|| {
        format!(
            "failed to resolve workspace {}",
            options.workspace_root.display()
        )
    })?;
    let mut state =
        capture_git_state(&requested).context("failed to capture workspace Git state")?;
    state
        .changes
        .retain(|change| !is_transaction_state_path(&change.path));
    if state.root != requested {
        bail!("edit workspace must be the root of its main Git worktree");
    }
    let head = run_git_value(
        &requested,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        options,
        cancellation,
    )?;
    if !matches!(head.len(), 40 | 64) || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Git returned an invalid HEAD object id");
    }
    let git_dir = run_git_path(
        &requested,
        &["rev-parse", "--absolute-git-dir"],
        options,
        cancellation,
    )?;
    let common_dir = run_git_path(
        &requested,
        &["rev-parse", "--git-common-dir"],
        options,
        cancellation,
    )?;
    let index = run_git_path(
        &requested,
        &["rev-parse", "--git-path", "index"],
        options,
        cancellation,
    )?;
    let object_dir = run_git_path(
        &requested,
        &["rev-parse", "--git-path", "objects"],
        options,
        cancellation,
    )?;
    let main_worktree = git_dir == common_dir && requested.join(".git").is_dir();
    if !main_worktree {
        bail!("editing the original project requires its main Git worktree");
    }
    let state_json = serde_json::to_vec(&state).context("failed to serialize Git state")?;
    let root_hash = canonical_root_hash(&requested)?;
    let digest = working_tree_digest(&root_hash, &head, &state_json);
    Ok(ObservedRepository {
        root: requested.clone(),
        root_hash,
        boundary: GitRepositoryBoundary {
            worktree_root: requested,
            git_dir,
            common_dir,
            index,
            object_dir,
            head: head.to_ascii_lowercase(),
            main_worktree,
        },
        working_tree_digest: digest,
        clean: state.changes.is_empty(),
    })
}

fn is_transaction_state_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    normalized == ".opticcode" || normalized.starts_with(".opticcode/")
}

fn run_git_path(
    root: &Path,
    args: &[&str],
    options: &EditRuntimeOptions,
    cancellation: &CancellationToken,
) -> Result<PathBuf> {
    let value = PathBuf::from(run_git_value(root, args, options, cancellation)?);
    let value = if value.is_absolute() {
        value
    } else {
        root.join(value)
    };
    fs::canonicalize(&value)
        .with_context(|| format!("failed to resolve Git path {}", value.display()))
}

fn run_git_value(
    root: &Path,
    args: &[&str],
    options: &EditRuntimeOptions,
    cancellation: &CancellationToken,
) -> Result<String> {
    let mut request = ProcessRequest::new("git", root);
    request.args = args.iter().map(OsString::from).collect();
    request.timeout = options.git_timeout;
    request.output_limit_bytes = options.output_limit_bytes.min(256 * 1024);
    request
        .environment
        .push((OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0")));
    let result = run_process_with_cancellation(&request, Some(cancellation))?;
    if !result.success() || result.output.output_truncated {
        bail!(
            "bounded Git observation failed: status={}, stderr={}",
            result.status.as_str(),
            result.stderr.trim()
        );
    }
    let value = result.stdout.trim();
    if value.is_empty() {
        bail!("bounded Git observation returned an empty value");
    }
    Ok(value.to_string())
}
