//! Disposable Git worktrees used to verify a patch without mutating the source worktree.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::apply_transaction::ApplyTransactionResult;
use crate::git_state::{capture_git_state, BuildGitReport, GitStateSnapshot};
use crate::process_runner::{
    run_process_with_cancellation, CancellationToken, ProcessOutputStats, ProcessRequest,
    ProcessResult, ProcessStatus, ProcessTermination, DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES,
    DEFAULT_PROCESS_TIMEOUT, MAX_PROCESS_OUTPUT_LIMIT_BYTES, MAX_PROCESS_TIMEOUT,
};
use crate::{
    apply_java_legacy_patch_in_place, build_java_project_with_cancellation, ApplyPlan,
    BuildOptions, BuildResult, PatchCheckResult,
};

pub const WORKTREE_VERIFICATION_SCHEMA_VERSION: u32 = 1;
pub const WORKTREE_LEASE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_WORKTREE_GIT_TIMEOUT_SECONDS: u64 = 3 * 60;

const WORKTREE_STORAGE_DIRECTORY: &str = "opticcode-worktrees";
const WORKTREE_RUNS_DIRECTORY: &str = "runs";
const WORKTREE_LEASES_DIRECTORY: &str = "leases";
const GIT_LIST_OUTPUT_LIMIT_BYTES: usize = 4 * 1024 * 1024;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub struct WorktreeVerificationOptions {
    pub build_timeout: Duration,
    pub git_timeout: Duration,
    pub output_limit_bytes: usize,
}

impl Default for WorktreeVerificationOptions {
    fn default() -> Self {
        Self {
            build_timeout: DEFAULT_PROCESS_TIMEOUT,
            git_timeout: Duration::from_secs(DEFAULT_WORKTREE_GIT_TIMEOUT_SECONDS),
            output_limit_bytes: DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeVerificationStatus {
    Passed,
    SetupFailed,
    ApplyFailed,
    BuildFailed,
    Cancelled,
    VerificationFailed,
    SourceChanged,
}

impl WorktreeVerificationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::SetupFailed => "setup_failed",
            Self::ApplyFailed => "apply_failed",
            Self::BuildFailed => "build_failed",
            Self::Cancelled => "cancelled",
            Self::VerificationFailed => "verification_failed",
            Self::SourceChanged => "source_changed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeOperationErrorKind {
    Precondition,
    InvalidRunId,
    Git,
    Storage,
}

#[derive(Debug)]
struct WorktreeOperationError {
    kind: WorktreeOperationErrorKind,
    message: String,
}

impl std::fmt::Display for WorktreeOperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WorktreeOperationError {}

pub fn worktree_operation_error_kind(error: &anyhow::Error) -> Option<WorktreeOperationErrorKind> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<WorktreeOperationError>())
        .map(|error| error.kind)
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeSourceReport {
    pub project: PathBuf,
    pub git_root: PathBuf,
    pub relative_project: PathBuf,
    pub commit_before: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_after: Option<String>,
    pub refs_fingerprint_before: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refs_fingerprint_after: Option<String>,
    pub refs_unchanged: bool,
    pub before: GitStateSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<GitStateSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_guard: Option<BuildGitReport>,
    pub unchanged: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundedCommandReport {
    pub command: String,
    pub success: bool,
    pub process_id: Option<u32>,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub output: ProcessOutputStats,
    pub termination: ProcessTermination,
}

impl BoundedCommandReport {
    fn from_process(command: String, process: &ProcessResult) -> Self {
        Self {
            command,
            success: process.success(),
            process_id: process.process_id,
            status: process.status,
            exit_code: process.exit_code,
            duration_ms: duration_ms(process.duration),
            stdout_tail: tail_lines(&process.stdout, 30),
            stderr_tail: tail_lines(&process.stderr, 30),
            output: process.output.clone(),
            termination: process.termination.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeApplyReport {
    pub success: bool,
    pub change_count: usize,
    pub files: Vec<String>,
    pub patch: String,
    pub patch_bytes: usize,
    pub patch_hash: String,
    pub patch_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<PatchCheckResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply: Option<PatchCheckResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<ApplyTransactionResult>,
}

impl WorktreeApplyReport {
    fn from_plan(plan: ApplyPlan) -> Self {
        let root = plan.proposal.root.clone();
        let patch = plan.proposal.combined_diff();
        Self {
            success: plan.success(),
            change_count: plan.proposal.changes.len(),
            files: plan
                .proposal
                .changes
                .iter()
                .map(|change| {
                    change
                        .path
                        .strip_prefix(&root)
                        .unwrap_or(&change.path)
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect(),
            patch_bytes: patch.len(),
            patch_hash: format!("blake3:{}:{}", patch.len(), blake3::hash(patch.as_bytes())),
            patch,
            patch_complete: true,
            check: plan.check,
            apply: plan.apply,
            transaction: plan.transaction,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeBuildReport {
    pub success: bool,
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub summary: Vec<String>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub process_id: Option<u32>,
    pub process_status: ProcessStatus,
    pub timed_out: bool,
    pub cancelled: bool,
    pub timeout_ms: u64,
    pub output: ProcessOutputStats,
    pub termination: ProcessTermination,
    pub git_guard: BuildGitReport,
}

impl WorktreeBuildReport {
    fn from_result(result: BuildResult) -> Self {
        Self {
            success: result.command_succeeded(),
            command: result.command,
            exit_code: result.exit_code,
            duration_ms: duration_ms(result.duration),
            summary: result.summary,
            stdout_tail: result.stdout_tail,
            stderr_tail: result.stderr_tail,
            process_id: result.process_id,
            process_status: result.process_status,
            timed_out: result.timed_out,
            cancelled: result.cancelled,
            timeout_ms: duration_ms(result.timeout),
            output: result.output,
            termination: result.termination,
            git_guard: result.git_report,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeDiffReport {
    pub command: BoundedCommandReport,
    pub content: String,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeCleanupReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub operation_success: bool,
    pub run_id: String,
    pub worktree: PathBuf,
    pub attempted: bool,
    pub already_cleaned: bool,
    pub success: bool,
    pub registered: Option<bool>,
    pub descriptor_removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<BoundedCommandReport>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeVerificationReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub operation_success: bool,
    pub verification_success: bool,
    pub cleanup_success: bool,
    pub lease_recovery_required: bool,
    pub status: WorktreeVerificationStatus,
    pub run_id: String,
    pub duration_ms: u64,
    pub source: WorktreeSourceReport,
    pub worktree_root: PathBuf,
    pub worktree_project: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_commit: Option<String>,
    pub worktree_detached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation: Option<BoundedCommandReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_before: Option<GitStateSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply: Option<WorktreeApplyReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<WorktreeBuildReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_after: Option<GitStateSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<WorktreeDiffReport>,
    pub cleanup: WorktreeCleanupReport,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl WorktreeVerificationReport {
    pub fn success(&self) -> bool {
        self.operation_success
    }

    pub fn to_display_string(&self) -> String {
        let mut output = String::new();
        output.push_str("Disposable worktree verification:\n");
        output.push_str(&format!("- run id: {}\n", self.run_id));
        output.push_str(&format!("- status: {}\n", self.status.as_str()));
        output.push_str(&format!(
            "- verification success: {}\n",
            self.verification_success
        ));
        output.push_str(&format!("- source: {}\n", self.source.project.display()));
        output.push_str(&format!("- commit: {}\n", self.source.commit_before));
        output.push_str(&format!("- source unchanged: {}\n", self.source.unchanged));
        output.push_str(&format!(
            "- refs unchanged: {}\n",
            self.source.refs_unchanged
        ));
        output.push_str(&format!("- detached HEAD: {}\n", self.worktree_detached));
        output.push_str(&format!(
            "- cleanup: {}\n",
            if self.cleanup_success {
                "ok"
            } else {
                "recovery required"
            }
        ));
        output.push_str(&format!(
            "- duration: {:.3}s\n",
            self.duration_ms as f64 / 1_000.0
        ));
        if let Some(apply) = &self.apply {
            output.push_str(&format!(
                "- apply: {}, {} file(s)\n",
                if apply.success { "ok" } else { "failed" },
                apply.change_count
            ));
        }
        if let Some(build) = &self.build {
            output.push_str(&format!(
                "- build: {} ({})\n",
                if build.success { "ok" } else { "failed" },
                build.process_status.as_str()
            ));
        }
        for error in &self.errors {
            output.push_str(&format!("Error: {error}\n"));
        }
        for warning in &self.warnings {
            output.push_str(&format!("Warning: {warning}\n"));
        }
        output
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeLease {
    pub schema_version: u32,
    pub run_id: String,
    pub process_id: u32,
    pub created_unix_ms: u64,
    pub source_git_root: PathBuf,
    pub source_project: PathBuf,
    pub source_commit: String,
    pub worktree_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeLeaseInspection {
    pub descriptor: PathBuf,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<WorktreeLease>,
    pub target_exists: bool,
    pub registered: Option<bool>,
    pub errors: Vec<String>,
}

impl WorktreeLeaseInspection {
    pub fn to_display_string(&self) -> String {
        match &self.lease {
            Some(lease) => format!(
                "{} valid={} exists={} registered={} path={}",
                lease.run_id,
                self.valid,
                self.target_exists,
                self.registered
                    .map_or("unknown".to_string(), |value| value.to_string()),
                lease.worktree_path.display()
            ),
            None => format!(
                "{} valid=false errors={}",
                self.descriptor.display(),
                self.errors.join("; ")
            ),
        }
    }
}

pub fn verify_java_legacy_patch_in_worktree(
    source_project: &Path,
    options: WorktreeVerificationOptions,
    cancellation: &CancellationToken,
) -> Result<WorktreeVerificationReport> {
    verify_in_disposable_worktree_with_apply(
        source_project,
        options,
        cancellation,
        |worktree_project| {
            apply_java_legacy_patch_in_place(worktree_project).map(WorktreeApplyReport::from_plan)
        },
    )
}

pub(crate) fn verify_in_disposable_worktree_with_apply<F>(
    source_project: &Path,
    options: WorktreeVerificationOptions,
    cancellation: &CancellationToken,
    mut apply_action: F,
) -> Result<WorktreeVerificationReport>
where
    F: FnMut(&Path) -> Result<WorktreeApplyReport>,
{
    let storage = WorktreeStorage::default_storage()?;
    verify_patch_with_storage(
        source_project,
        options,
        cancellation,
        storage,
        &mut apply_action,
    )
}

pub fn list_disposable_worktrees() -> Result<Vec<WorktreeLeaseInspection>> {
    let storage = WorktreeStorage::default_storage()?;
    list_disposable_worktrees_in(&storage)
}

/// Inspects existing OpticCode worktree leases without creating storage directories.
pub fn inspect_disposable_worktrees_read_only() -> Result<Vec<WorktreeLeaseInspection>> {
    let Some(storage) = WorktreeStorage::open_existing()? else {
        return Ok(Vec::new());
    };
    list_disposable_worktrees_in(&storage)
}

pub fn cleanup_disposable_worktree(run_id: &str) -> Result<WorktreeCleanupReport> {
    validate_run_id(run_id)?;
    let storage = WorktreeStorage::default_storage()?;
    cleanup_disposable_worktree_in(&storage, run_id)
}

fn cleanup_disposable_worktree_in(
    storage: &WorktreeStorage,
    run_id: &str,
) -> Result<WorktreeCleanupReport> {
    validate_run_id(run_id)?;
    let descriptor = storage.lease_path(run_id);
    match fs::symlink_metadata(&descriptor) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let worktree = storage.worktree_path(run_id);
            let target_exists = worktree.exists();
            return Ok(WorktreeCleanupReport {
                schema_version: WORKTREE_VERIFICATION_SCHEMA_VERSION,
                operation: "worktree_cleanup",
                operation_success: !target_exists,
                run_id: run_id.to_string(),
                worktree,
                attempted: false,
                already_cleaned: !target_exists,
                success: !target_exists,
                registered: None,
                descriptor_removed: true,
                command: None,
                errors: if target_exists {
                    vec![
                        "worktree path exists without its lease; refusing cleanup because the source repository cannot be verified"
                            .to_string(),
                    ]
                } else {
                    Vec::new()
                },
            });
        }
        Err(error) => {
            return Err(operation_error(
                WorktreeOperationErrorKind::Storage,
                format!(
                    "failed to inspect worktree lease {}: {error}",
                    descriptor.display()
                ),
            ));
        }
    }
    let lease = read_lease(storage, run_id)?;
    cleanup_lease(
        storage,
        &lease,
        Duration::from_secs(DEFAULT_WORKTREE_GIT_TIMEOUT_SECONDS),
        DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES,
    )
}

fn verify_patch_with_storage<F>(
    source_project: &Path,
    options: WorktreeVerificationOptions,
    cancellation: &CancellationToken,
    storage: WorktreeStorage,
    apply_action: &mut F,
) -> Result<WorktreeVerificationReport>
where
    F: FnMut(&Path) -> Result<WorktreeApplyReport>,
{
    let started_at = Instant::now();
    validate_options(options)?;
    let source_project = fs::canonicalize(source_project).map_err(|error| {
        operation_error(
            WorktreeOperationErrorKind::Precondition,
            format!(
                "failed to resolve source project {}: {error}",
                source_project.display()
            ),
        )
    })?;
    if !source_project.is_dir() {
        return Err(operation_error(
            WorktreeOperationErrorKind::Precondition,
            format!(
                "source project is not a directory: {}",
                source_project.display()
            ),
        ));
    }

    let source_before = capture_git_state(&source_project).map_err(|error| {
        operation_error(
            WorktreeOperationErrorKind::Precondition,
            format!("source project must be a Git worktree: {error:#}"),
        )
    })?;
    if !source_before.changes.is_empty() {
        return Err(operation_error(
            WorktreeOperationErrorKind::Precondition,
            format!(
                "source Git worktree must be clean; found {} change(s)",
                source_before.changes.len()
            ),
        ));
    }
    let git_root = source_before.root.clone();
    let relative_project = source_project
        .strip_prefix(&git_root)
        .map_err(|_| {
            operation_error(
                WorktreeOperationErrorKind::Precondition,
                format!(
                    "source project {} is outside Git root {}",
                    source_project.display(),
                    git_root.display()
                ),
            )
        })?
        .to_path_buf();
    let commit_before = resolve_head(
        &git_root,
        options.git_timeout,
        options.output_limit_bytes,
        Some(cancellation),
    )?;
    let refs_fingerprint_before = capture_refs_fingerprint(
        &git_root,
        options.git_timeout,
        options.output_limit_bytes,
        Some(cancellation),
    )?;
    let run_id = new_run_id();
    let worktree_root = storage.worktree_path(&run_id);
    let worktree_project = worktree_root.join(&relative_project);
    let lease = WorktreeLease {
        schema_version: WORKTREE_LEASE_SCHEMA_VERSION,
        run_id: run_id.clone(),
        process_id: std::process::id(),
        created_unix_ms: unix_millis(),
        source_git_root: git_root.clone(),
        source_project: source_project.clone(),
        source_commit: commit_before.clone(),
        worktree_path: worktree_root.clone(),
    };
    write_lease(&storage, &lease)?;
    if let Err(error) = fs::create_dir(&worktree_root) {
        let _ = fs::remove_file(storage.lease_path(&run_id));
        return Err(operation_error(
            WorktreeOperationErrorKind::Storage,
            format!(
                "failed to reserve disposable worktree {}: {error}",
                worktree_root.display()
            ),
        ));
    }

    let mut report = WorktreeVerificationReport {
        schema_version: WORKTREE_VERIFICATION_SCHEMA_VERSION,
        operation: "worktree_verify",
        operation_success: false,
        verification_success: false,
        cleanup_success: false,
        lease_recovery_required: false,
        status: WorktreeVerificationStatus::SetupFailed,
        run_id: run_id.clone(),
        duration_ms: 0,
        source: WorktreeSourceReport {
            project: source_project,
            git_root: git_root.clone(),
            relative_project,
            commit_before: commit_before.clone(),
            commit_after: None,
            refs_fingerprint_before,
            refs_fingerprint_after: None,
            refs_unchanged: false,
            before: source_before,
            after: None,
            git_guard: None,
            unchanged: false,
        },
        worktree_root: worktree_root.clone(),
        worktree_project: worktree_project.clone(),
        worktree_commit: None,
        worktree_detached: false,
        creation: None,
        worktree_before: None,
        apply: None,
        build: None,
        worktree_after: None,
        diff: None,
        cleanup: WorktreeCleanupReport {
            schema_version: WORKTREE_VERIFICATION_SCHEMA_VERSION,
            operation: "worktree_cleanup",
            operation_success: false,
            run_id: run_id.clone(),
            worktree: worktree_root.clone(),
            attempted: false,
            already_cleaned: false,
            success: false,
            registered: None,
            descriptor_removed: false,
            command: None,
            errors: Vec::new(),
        },
        errors: Vec::new(),
        warnings: Vec::new(),
    };

    match add_worktree(
        &git_root,
        &worktree_root,
        &commit_before,
        options,
        cancellation,
    ) {
        Ok((process, command)) => {
            let creation = BoundedCommandReport::from_process(command, &process);
            let created = creation.success;
            let cancelled = creation.status == ProcessStatus::Cancelled;
            report.creation = Some(creation);
            if created {
                run_verification_pipeline(&mut report, options, cancellation, apply_action);
            } else {
                report.status = if cancelled {
                    WorktreeVerificationStatus::Cancelled
                } else {
                    WorktreeVerificationStatus::SetupFailed
                };
                report.errors.push(format!(
                    "git worktree add failed: {}",
                    process_error_summary(&process)
                ));
            }
        }
        Err(error) => report
            .errors
            .push(format!("failed to start git worktree add: {error:#}")),
    }

    if worktree_root.exists() {
        match capture_git_state(&worktree_root) {
            Ok(snapshot) => report.worktree_after = Some(snapshot),
            Err(error) => {
                report
                    .errors
                    .push(format!("failed to capture final worktree state: {error:#}"));
                if report.status == WorktreeVerificationStatus::Passed {
                    report.status = WorktreeVerificationStatus::VerificationFailed;
                }
            }
        }
        match capture_worktree_diff(&worktree_root, options, cancellation) {
            Ok(diff) => {
                if !diff.command.success {
                    report.errors.push(format!(
                        "final worktree diff command failed: {}",
                        if diff.command.stderr_tail.trim().is_empty() {
                            format!("process status {}", diff.command.status.as_str())
                        } else {
                            diff.command.stderr_tail.trim().to_string()
                        }
                    ));
                    if report.status == WorktreeVerificationStatus::Passed {
                        report.status = WorktreeVerificationStatus::VerificationFailed;
                    }
                } else if !diff.complete {
                    report.warnings.push(
                        "final diff content exceeded the JSON output limit; worktree_after remains the authoritative path/status/hash summary"
                            .to_string(),
                    );
                }
                report.diff = Some(diff);
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("failed to capture final worktree diff: {error:#}"));
                if report.status == WorktreeVerificationStatus::Passed {
                    report.status = WorktreeVerificationStatus::VerificationFailed;
                }
            }
        }
    }

    report.cleanup = cleanup_lease(
        &storage,
        &lease,
        options.git_timeout,
        options.output_limit_bytes,
    )?;
    if !report.cleanup.success {
        report.errors.extend(report.cleanup.errors.iter().cloned());
    }

    match capture_git_state(&git_root) {
        Ok(after) => {
            match BuildGitReport::from_snapshots(report.source.before.clone(), after.clone(), true)
            {
                Ok(guard) => report.source.git_guard = Some(guard),
                Err(error) => report
                    .errors
                    .push(format!("failed to compare source Git states: {error:#}")),
            }
            report.source.after = Some(after);
        }
        Err(error) => report
            .errors
            .push(format!("failed to recapture source Git state: {error:#}")),
    }
    match resolve_head(
        &git_root,
        options.git_timeout,
        options.output_limit_bytes,
        None,
    ) {
        Ok(commit) => report.source.commit_after = Some(commit),
        Err(error) => report
            .errors
            .push(format!("failed to recapture source HEAD: {error:#}")),
    }
    match capture_refs_fingerprint(
        &git_root,
        options.git_timeout,
        options.output_limit_bytes,
        None,
    ) {
        Ok(fingerprint) => report.source.refs_fingerprint_after = Some(fingerprint),
        Err(error) => report
            .errors
            .push(format!("failed to recapture source refs: {error:#}")),
    }

    report.source.refs_unchanged = report
        .source
        .refs_fingerprint_after
        .as_ref()
        .is_some_and(|after| after == &report.source.refs_fingerprint_before);
    let commit_unchanged = report
        .source
        .commit_after
        .as_ref()
        .is_some_and(|after| after == &report.source.commit_before);
    let guard_passed = report
        .source
        .git_guard
        .as_ref()
        .is_some_and(|guard| !guard.strict_violation());
    report.source.unchanged = commit_unchanged && report.source.refs_unchanged && guard_passed;
    if !report.source.unchanged {
        report.status = WorktreeVerificationStatus::SourceChanged;
    }

    let summary = verification_summary(
        report.status,
        report.source.unchanged,
        report.cleanup.success,
    );
    report.verification_success = summary.verification_success;
    report.cleanup_success = summary.cleanup_success;
    report.lease_recovery_required = summary.lease_recovery_required;
    report.operation_success = summary.operation_success;
    report.duration_ms = duration_ms(started_at.elapsed());
    Ok(report)
}

fn run_verification_pipeline<F>(
    report: &mut WorktreeVerificationReport,
    options: WorktreeVerificationOptions,
    cancellation: &CancellationToken,
    apply_action: &mut F,
) where
    F: FnMut(&Path) -> Result<WorktreeApplyReport>,
{
    match capture_git_state(&report.worktree_root) {
        Ok(snapshot) if snapshot.changes.is_empty() => report.worktree_before = Some(snapshot),
        Ok(snapshot) => {
            report.errors.push(format!(
                "new disposable worktree was not clean; found {} change(s)",
                snapshot.changes.len()
            ));
            report.worktree_before = Some(snapshot);
            return;
        }
        Err(error) => {
            report
                .errors
                .push(format!("failed to capture new worktree state: {error:#}"));
            return;
        }
    }

    if cancellation.is_cancelled() {
        report.status = WorktreeVerificationStatus::Cancelled;
        return;
    }

    match resolve_head(
        &report.worktree_root,
        options.git_timeout,
        options.output_limit_bytes,
        Some(cancellation),
    ) {
        Ok(commit) if commit == report.source.commit_before => {
            report.worktree_commit = Some(commit);
        }
        Ok(commit) => {
            report.worktree_commit = Some(commit.clone());
            report.errors.push(format!(
                "disposable worktree commit mismatch: expected {}, got {commit}",
                report.source.commit_before
            ));
            return;
        }
        Err(error) => {
            report.errors.push(format!(
                "failed to verify disposable worktree commit: {error:#}"
            ));
            return;
        }
    }

    match head_is_detached(
        &report.worktree_root,
        options.git_timeout,
        options.output_limit_bytes,
        Some(cancellation),
    ) {
        Ok(true) => report.worktree_detached = true,
        Ok(false) => {
            report
                .errors
                .push("disposable worktree unexpectedly attached to a branch".to_string());
            return;
        }
        Err(error) => {
            report
                .errors
                .push(format!("failed to verify detached HEAD: {error:#}"));
            return;
        }
    }

    match apply_action(&report.worktree_project) {
        Ok(apply) => {
            let success = apply.success;
            report.apply = Some(apply);
            if !success {
                report.status = WorktreeVerificationStatus::ApplyFailed;
                return;
            }
        }
        Err(error) => {
            report.status = WorktreeVerificationStatus::ApplyFailed;
            report
                .errors
                .push(format!("worktree apply pipeline failed: {error:#}"));
            return;
        }
    }

    if cancellation.is_cancelled() {
        report.status = WorktreeVerificationStatus::Cancelled;
        return;
    }

    match build_java_project_with_cancellation(
        &report.worktree_project,
        BuildOptions {
            fail_on_worktree_change: true,
            timeout: options.build_timeout,
            output_limit_bytes: options.output_limit_bytes,
        },
        cancellation,
    ) {
        Ok(result) => {
            let build = WorktreeBuildReport::from_result(result);
            report.status = if build.success {
                WorktreeVerificationStatus::Passed
            } else if build.cancelled {
                WorktreeVerificationStatus::Cancelled
            } else {
                WorktreeVerificationStatus::BuildFailed
            };
            report.build = Some(build);
        }
        Err(error) => {
            report.status = WorktreeVerificationStatus::BuildFailed;
            report
                .errors
                .push(format!("bounded build failed: {error:#}"));
        }
    }
}

fn add_worktree(
    git_root: &Path,
    worktree_root: &Path,
    commit: &str,
    options: WorktreeVerificationOptions,
    cancellation: &CancellationToken,
) -> Result<(ProcessResult, String)> {
    let args = vec![
        OsString::from("worktree"),
        OsString::from("add"),
        OsString::from("--detach"),
        worktree_root.as_os_str().to_os_string(),
        OsString::from(commit),
    ];
    let command = format!(
        "git worktree add --detach {} {}",
        quote_path(worktree_root),
        commit
    );
    run_git(
        git_root,
        args,
        options.git_timeout,
        options.output_limit_bytes,
        Some(cancellation),
    )
    .map(|process| (process, command))
}

fn capture_worktree_diff(
    worktree_root: &Path,
    options: WorktreeVerificationOptions,
    cancellation: &CancellationToken,
) -> Result<WorktreeDiffReport> {
    let command_text = "git --no-pager diff --binary --no-ext-diff --no-color HEAD".to_string();
    let process = run_git(
        worktree_root,
        [
            "--no-pager",
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-color",
            "HEAD",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
        options.git_timeout,
        options.output_limit_bytes,
        Some(cancellation),
    )?;
    let content = process.stdout.clone();
    let complete = process.success() && !process.output.output_truncated;
    Ok(WorktreeDiffReport {
        command: BoundedCommandReport::from_process(command_text, &process),
        content,
        complete,
    })
}

fn resolve_head(
    git_root: &Path,
    timeout: Duration,
    output_limit_bytes: usize,
    cancellation: Option<&CancellationToken>,
) -> Result<String> {
    let process = run_git(
        git_root,
        ["rev-parse", "--verify", "HEAD^{commit}"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        timeout,
        output_limit_bytes,
        cancellation,
    )
    .map_err(|error| {
        operation_error(
            WorktreeOperationErrorKind::Git,
            format!("failed to start git rev-parse: {error:#}"),
        )
    })?;
    if !process.success() {
        return Err(operation_error(
            WorktreeOperationErrorKind::Git,
            format!(
                "failed to resolve source HEAD: {}",
                process_error_summary(&process)
            ),
        ));
    }
    if process.output.stdout_truncated {
        return Err(operation_error(
            WorktreeOperationErrorKind::Git,
            "git rev-parse output was unexpectedly truncated",
        ));
    }
    let commit = process.stdout.trim();
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(operation_error(
            WorktreeOperationErrorKind::Git,
            format!("git rev-parse returned an invalid commit id: {commit:?}"),
        ));
    }
    Ok(commit.to_ascii_lowercase())
}

fn capture_refs_fingerprint(
    git_root: &Path,
    timeout: Duration,
    output_limit_bytes: usize,
    cancellation: Option<&CancellationToken>,
) -> Result<String> {
    let process = run_git(
        git_root,
        [
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)%00%(objectname)%00%(symref)",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
        timeout,
        output_limit_bytes,
        cancellation,
    )
    .map_err(|error| {
        operation_error(
            WorktreeOperationErrorKind::Git,
            format!("failed to start git for-each-ref: {error:#}"),
        )
    })?;
    if !process.success() {
        return Err(operation_error(
            WorktreeOperationErrorKind::Git,
            format!(
                "failed to capture source refs: {}",
                process_error_summary(&process)
            ),
        ));
    }
    if process.output.stdout_truncated {
        return Err(operation_error(
            WorktreeOperationErrorKind::Git,
            "git for-each-ref output exceeded the configured output limit",
        ));
    }
    Ok(format!(
        "blake3:{}:{}",
        process.stdout.len(),
        blake3::hash(process.stdout.as_bytes()).to_hex()
    ))
}

fn head_is_detached(
    worktree_root: &Path,
    timeout: Duration,
    output_limit_bytes: usize,
    cancellation: Option<&CancellationToken>,
) -> Result<bool> {
    let process = run_git(
        worktree_root,
        ["symbolic-ref", "--quiet", "HEAD"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        timeout,
        output_limit_bytes,
        cancellation,
    )?;
    if process.success() {
        return Ok(false);
    }
    if process.status == ProcessStatus::Failed && process.exit_code == Some(1) {
        return Ok(true);
    }
    Err(operation_error(
        WorktreeOperationErrorKind::Git,
        format!(
            "git symbolic-ref could not determine HEAD state: {}",
            process_error_summary(&process)
        ),
    ))
}

fn run_git(
    working_directory: &Path,
    args: Vec<OsString>,
    timeout: Duration,
    output_limit_bytes: usize,
    cancellation: Option<&CancellationToken>,
) -> Result<ProcessResult> {
    let mut request = ProcessRequest::new("git", working_directory);
    request.args = args;
    request.timeout = timeout;
    request.output_limit_bytes = output_limit_bytes;
    request
        .environment
        .push((OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0")));
    run_process_with_cancellation(&request, cancellation)
}

fn cleanup_lease(
    storage: &WorktreeStorage,
    lease: &WorktreeLease,
    timeout: Duration,
    output_limit_bytes: usize,
) -> Result<WorktreeCleanupReport> {
    validate_lease(storage, lease)?;
    let mut report = WorktreeCleanupReport {
        schema_version: WORKTREE_VERIFICATION_SCHEMA_VERSION,
        operation: "worktree_cleanup",
        operation_success: false,
        run_id: lease.run_id.clone(),
        worktree: lease.worktree_path.clone(),
        attempted: true,
        already_cleaned: false,
        success: false,
        registered: None,
        descriptor_removed: false,
        command: None,
        errors: Vec::new(),
    };

    let registered = match worktree_is_registered(
        &lease.source_git_root,
        &lease.worktree_path,
        timeout,
        output_limit_bytes.max(GIT_LIST_OUTPUT_LIMIT_BYTES),
    ) {
        Ok(registered) => registered,
        Err(error) => {
            report.errors.push(format!(
                "failed to verify Git worktree registration: {error:#}"
            ));
            return Ok(report);
        }
    };
    report.registered = Some(registered);

    if registered {
        let args = vec![
            OsString::from("worktree"),
            OsString::from("remove"),
            OsString::from("--force"),
            lease.worktree_path.as_os_str().to_os_string(),
        ];
        let command_text = format!(
            "git worktree remove --force {}",
            quote_path(&lease.worktree_path)
        );
        match run_git(
            &lease.source_git_root,
            args,
            timeout,
            output_limit_bytes,
            None,
        ) {
            Ok(process) => {
                let command = BoundedCommandReport::from_process(command_text, &process);
                let success = command.success;
                if !success {
                    report.errors.push(format!(
                        "git worktree remove failed: {}",
                        process_error_summary(&process)
                    ));
                }
                report.command = Some(command);
                if !success {
                    return Ok(report);
                }
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("failed to start git worktree remove: {error:#}"));
                return Ok(report);
            }
        }
    } else if lease.worktree_path.exists() {
        if !directory_is_empty(&lease.worktree_path)? {
            report.errors.push(format!(
                "refusing to remove unregistered non-empty directory {}",
                lease.worktree_path.display()
            ));
            return Ok(report);
        }
        if let Err(error) = fs::remove_dir(&lease.worktree_path) {
            report.errors.push(format!(
                "failed to remove empty reserved directory {}: {error}",
                lease.worktree_path.display()
            ));
            return Ok(report);
        }
    }

    if lease.worktree_path.exists() {
        report.errors.push(format!(
            "worktree path still exists after cleanup: {}",
            lease.worktree_path.display()
        ));
        return Ok(report);
    }

    let still_registered = match worktree_is_registered(
        &lease.source_git_root,
        &lease.worktree_path,
        timeout,
        output_limit_bytes.max(GIT_LIST_OUTPUT_LIMIT_BYTES),
    ) {
        Ok(registered) => registered,
        Err(error) => {
            report.errors.push(format!(
                "failed to verify worktree removal in Git metadata: {error:#}"
            ));
            return Ok(report);
        }
    };
    if still_registered {
        report
            .errors
            .push("worktree remains registered after cleanup".to_string());
        return Ok(report);
    }

    let descriptor = storage.lease_path(&lease.run_id);
    match fs::remove_file(&descriptor) {
        Ok(()) => report.descriptor_removed = true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            report.descriptor_removed = true;
        }
        Err(error) => {
            report.errors.push(format!(
                "worktree was removed, but lease descriptor {} could not be removed: {error}",
                descriptor.display()
            ));
            return Ok(report);
        }
    }
    report.success = true;
    report.operation_success = true;
    Ok(report)
}

fn worktree_is_registered(
    source_git_root: &Path,
    target: &Path,
    timeout: Duration,
    output_limit_bytes: usize,
) -> Result<bool> {
    let process = run_git(
        source_git_root,
        ["worktree", "list", "--porcelain", "-z"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        timeout,
        output_limit_bytes,
        None,
    )?;
    if !process.success() {
        anyhow::bail!(
            "git worktree list failed: {}",
            process_error_summary(&process)
        );
    }
    if process.output.output_truncated {
        anyhow::bail!("git worktree list output was truncated");
    }

    Ok(process
        .stdout
        .split('\0')
        .filter_map(|field| field.strip_prefix("worktree "))
        .map(PathBuf::from)
        .any(|path| paths_match(&path, target)))
}

fn list_disposable_worktrees_in(storage: &WorktreeStorage) -> Result<Vec<WorktreeLeaseInspection>> {
    let mut descriptors = fs::read_dir(&storage.leases)
        .with_context(|| {
            format!(
                "failed to list worktree leases: {}",
                storage.leases.display()
            )
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension() == Some(OsStr::new("json")))
        .collect::<Vec<_>>();
    descriptors.sort();

    Ok(descriptors
        .into_iter()
        .map(|descriptor| inspect_lease(storage, descriptor))
        .collect())
}

fn inspect_lease(storage: &WorktreeStorage, descriptor: PathBuf) -> WorktreeLeaseInspection {
    let mut inspection = WorktreeLeaseInspection {
        descriptor: descriptor.clone(),
        valid: false,
        lease: None,
        target_exists: false,
        registered: None,
        errors: Vec::new(),
    };
    if path_is_reparse_point(&descriptor).unwrap_or(true) {
        inspection
            .errors
            .push("lease descriptor is a symlink or reparse point".to_string());
        return inspection;
    }
    let file = match File::open(&descriptor) {
        Ok(file) => file,
        Err(error) => {
            inspection
                .errors
                .push(format!("failed to open descriptor: {error}"));
            return inspection;
        }
    };
    let lease: WorktreeLease = match serde_json::from_reader(BufReader::new(file)) {
        Ok(lease) => lease,
        Err(error) => {
            inspection
                .errors
                .push(format!("invalid lease JSON: {error}"));
            return inspection;
        }
    };
    inspection.target_exists = lease.worktree_path.exists();
    if let Err(error) = validate_lease(storage, &lease) {
        inspection.errors.push(format!("{error:#}"));
        inspection.lease = Some(lease);
        return inspection;
    }
    match worktree_is_registered(
        &lease.source_git_root,
        &lease.worktree_path,
        Duration::from_secs(DEFAULT_WORKTREE_GIT_TIMEOUT_SECONDS),
        GIT_LIST_OUTPUT_LIMIT_BYTES,
    ) {
        Ok(registered) => inspection.registered = Some(registered),
        Err(error) => inspection
            .errors
            .push(format!("could not inspect Git registration: {error:#}")),
    }
    inspection.valid = inspection.errors.is_empty();
    inspection.lease = Some(lease);
    inspection
}

fn read_lease(storage: &WorktreeStorage, run_id: &str) -> Result<WorktreeLease> {
    validate_run_id(run_id)?;
    let path = storage.lease_path(run_id);
    if path_is_reparse_point(&path).unwrap_or(false) {
        return Err(operation_error(
            WorktreeOperationErrorKind::Storage,
            format!(
                "lease descriptor is a symlink or reparse point: {}",
                path.display()
            ),
        ));
    }
    let file = File::open(&path).map_err(|error| {
        operation_error(
            WorktreeOperationErrorKind::InvalidRunId,
            format!("failed to open worktree lease {}: {error}", path.display()),
        )
    })?;
    let lease: WorktreeLease = serde_json::from_reader(BufReader::new(file)).map_err(|error| {
        operation_error(
            WorktreeOperationErrorKind::Storage,
            format!("invalid worktree lease {}: {error}", path.display()),
        )
    })?;
    validate_lease(storage, &lease)?;
    Ok(lease)
}

fn write_lease(storage: &WorktreeStorage, lease: &WorktreeLease) -> Result<()> {
    validate_run_id(&lease.run_id)?;
    let final_path = storage.lease_path(&lease.run_id);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&final_path)
        .with_context(|| format!("failed to create worktree lease: {}", final_path.display()))?;
    let mut writer = BufWriter::new(file);
    let write_result = (|| -> Result<()> {
        serde_json::to_writer_pretty(&mut writer, lease)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&final_path);
    }
    write_result
}

fn validate_lease(storage: &WorktreeStorage, lease: &WorktreeLease) -> Result<()> {
    validate_run_id(&lease.run_id)?;
    if lease.schema_version != WORKTREE_LEASE_SCHEMA_VERSION {
        return Err(operation_error(
            WorktreeOperationErrorKind::Storage,
            format!(
                "unsupported worktree lease schema version {}",
                lease.schema_version
            ),
        ));
    }
    let expected = storage.worktree_path(&lease.run_id);
    if !paths_match_without_canonicalization(&expected, &lease.worktree_path) {
        return Err(operation_error(
            WorktreeOperationErrorKind::Storage,
            format!(
                "lease worktree path does not match controlled path: {} != {}",
                lease.worktree_path.display(),
                expected.display()
            ),
        ));
    }
    if lease.worktree_path.exists() {
        let resolved =
            normalize_verbatim_path(fs::canonicalize(&lease.worktree_path).with_context(|| {
                format!(
                    "failed to resolve leased worktree path: {}",
                    lease.worktree_path.display()
                )
            })?);
        if !resolved.starts_with(&storage.runs) || resolved == storage.runs {
            return Err(operation_error(
                WorktreeOperationErrorKind::Storage,
                format!(
                    "leased worktree escapes controlled storage: {}",
                    resolved.display()
                ),
            ));
        }
        if path_is_reparse_point(&lease.worktree_path)? {
            return Err(operation_error(
                WorktreeOperationErrorKind::Storage,
                format!(
                    "leased worktree is a symlink or reparse point: {}",
                    lease.worktree_path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn validate_run_id(run_id: &str) -> Result<()> {
    let valid = (1..=96).contains(&run_id.len())
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !valid {
        return Err(operation_error(
            WorktreeOperationErrorKind::InvalidRunId,
            "worktree run id must contain only ASCII letters, digits, '-' or '_'",
        ));
    }
    Ok(())
}

fn validate_options(options: WorktreeVerificationOptions) -> Result<()> {
    if options.build_timeout.is_zero() || options.git_timeout.is_zero() {
        return Err(operation_error(
            WorktreeOperationErrorKind::Precondition,
            "worktree and build timeouts must be greater than zero",
        ));
    }
    if options.build_timeout > MAX_PROCESS_TIMEOUT || options.git_timeout > MAX_PROCESS_TIMEOUT {
        return Err(operation_error(
            WorktreeOperationErrorKind::Precondition,
            format!(
                "worktree and build timeouts cannot exceed {} seconds",
                MAX_PROCESS_TIMEOUT.as_secs()
            ),
        ));
    }
    if options.output_limit_bytes == 0 {
        return Err(operation_error(
            WorktreeOperationErrorKind::Precondition,
            "process output limit must be greater than zero",
        ));
    }
    if options.output_limit_bytes > MAX_PROCESS_OUTPUT_LIMIT_BYTES {
        return Err(operation_error(
            WorktreeOperationErrorKind::Precondition,
            format!(
                "process output limit cannot exceed {} bytes per stream",
                MAX_PROCESS_OUTPUT_LIMIT_BYTES
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct WorktreeStorage {
    runs: PathBuf,
    leases: PathBuf,
}

impl WorktreeStorage {
    fn default_storage() -> Result<Self> {
        let temp = normalize_verbatim_path(
            fs::canonicalize(std::env::temp_dir())
                .context("failed to resolve the system temporary directory")?,
        );
        let base = temp.join(WORKTREE_STORAGE_DIRECTORY);
        Self::prepare(&base, Some(&temp))
    }

    fn open_existing() -> Result<Option<Self>> {
        let temp = normalize_verbatim_path(
            fs::canonicalize(std::env::temp_dir())
                .context("failed to resolve the system temporary directory")?,
        );
        let base = temp.join(WORKTREE_STORAGE_DIRECTORY);
        match fs::symlink_metadata(&base) {
            Ok(metadata) if metadata.is_dir() && !path_is_reparse_point(&base)? => {}
            Ok(_) => {
                return Err(operation_error(
                    WorktreeOperationErrorKind::Storage,
                    format!(
                        "worktree storage is not a normal directory: {}",
                        base.display()
                    ),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(operation_error(
                    WorktreeOperationErrorKind::Storage,
                    format!(
                        "failed to inspect worktree storage {}: {error}",
                        base.display()
                    ),
                ));
            }
        }

        let base = normalize_verbatim_path(fs::canonicalize(&base)?);
        if base.parent() != Some(temp.as_path()) {
            return Err(operation_error(
                WorktreeOperationErrorKind::Storage,
                format!(
                    "worktree storage is not a direct child of the temporary directory: {}",
                    base.display()
                ),
            ));
        }

        let runs = open_existing_controlled_directory(&base.join(WORKTREE_RUNS_DIRECTORY), &base)?;
        let leases =
            open_existing_controlled_directory(&base.join(WORKTREE_LEASES_DIRECTORY), &base)?;
        Ok(Some(Self { runs, leases }))
    }

    fn prepare(base: &Path, required_parent: Option<&Path>) -> Result<Self> {
        create_controlled_directory(base)?;
        let base =
            normalize_verbatim_path(fs::canonicalize(base).with_context(|| {
                format!("failed to resolve worktree storage: {}", base.display())
            })?);
        if let Some(parent) = required_parent {
            if base.parent() != Some(parent) {
                return Err(operation_error(
                    WorktreeOperationErrorKind::Storage,
                    format!(
                        "worktree storage is not a direct child of the temporary directory: {}",
                        base.display()
                    ),
                ));
            }
        }
        let runs = base.join(WORKTREE_RUNS_DIRECTORY);
        let leases = base.join(WORKTREE_LEASES_DIRECTORY);
        create_controlled_directory(&runs)?;
        create_controlled_directory(&leases)?;
        let runs = normalize_verbatim_path(fs::canonicalize(&runs)?);
        let leases = normalize_verbatim_path(fs::canonicalize(&leases)?);
        if runs.parent() != Some(base.as_path()) || leases.parent() != Some(base.as_path()) {
            return Err(operation_error(
                WorktreeOperationErrorKind::Storage,
                "worktree storage subdirectories escaped their controlled parent",
            ));
        }
        Ok(Self { runs, leases })
    }

    fn worktree_path(&self, run_id: &str) -> PathBuf {
        self.runs.join(run_id)
    }

    fn lease_path(&self, run_id: &str) -> PathBuf {
        self.leases.join(format!("{run_id}.json"))
    }
}

fn open_existing_controlled_directory(path: &Path, parent: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "failed to inspect controlled worktree directory: {}",
            path.display()
        )
    })?;
    if !metadata.is_dir() || path_is_reparse_point(path)? {
        return Err(operation_error(
            WorktreeOperationErrorKind::Storage,
            format!(
                "controlled worktree path is not a normal directory: {}",
                path.display()
            ),
        ));
    }
    let resolved = normalize_verbatim_path(fs::canonicalize(path)?);
    if resolved.parent() != Some(parent) {
        return Err(operation_error(
            WorktreeOperationErrorKind::Storage,
            format!(
                "controlled worktree directory escaped its parent: {}",
                resolved.display()
            ),
        ));
    }
    Ok(resolved)
}

fn create_controlled_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(operation_error(
                WorktreeOperationErrorKind::Storage,
                format!(
                    "failed to create controlled directory {}: {error}",
                    path.display()
                ),
            ));
        }
    }
    if !path.is_dir() || path_is_reparse_point(path)? {
        return Err(operation_error(
            WorktreeOperationErrorKind::Storage,
            format!(
                "controlled directory is not a normal directory: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn path_is_reparse_point(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect path metadata: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }

    #[cfg(not(windows))]
    Ok(false)
}

fn directory_is_empty(path: &Path) -> Result<bool> {
    Ok(fs::read_dir(path)
        .with_context(|| format!("failed to inspect directory: {}", path.display()))?
        .next()
        .is_none())
}

fn new_run_id() -> String {
    format!(
        "verify-{}-{}-{}",
        unix_millis(),
        std::process::id(),
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn process_error_summary(process: &ProcessResult) -> String {
    let stderr = process.stderr.trim();
    let stdout = process.stdout.trim();
    if !stderr.is_empty() {
        stderr.to_string()
    } else if !stdout.is_empty() {
        stdout.to_string()
    } else {
        format!(
            "status={}, exit_code={}",
            process.status.as_str(),
            process
                .exit_code
                .map_or("unknown".to_string(), |code| code.to_string())
        )
    }
}

fn tail_lines(value: &str, limit: usize) -> String {
    let lines = value.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(limit)..].join("\n")
}

fn quote_path(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

fn paths_match(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => paths_match_without_canonicalization(
            &normalize_verbatim_path(left),
            &normalize_verbatim_path(right),
        ),
        _ => paths_match_without_canonicalization(left, right),
    }
}

fn normalize_verbatim_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        const VERBATIM_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
        const UNC_PREFIX: &[u16] = &[
            b'\\' as u16,
            b'\\' as u16,
            b'?' as u16,
            b'\\' as u16,
            b'U' as u16,
            b'N' as u16,
            b'C' as u16,
            b'\\' as u16,
        ];

        let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if wide.starts_with(UNC_PREFIX) {
            let mut normalized = vec![b'\\' as u16, b'\\' as u16];
            normalized.extend_from_slice(&wide[UNC_PREFIX.len()..]);
            return PathBuf::from(OsString::from_wide(&normalized));
        }
        if wide.starts_with(VERBATIM_PREFIX) {
            return PathBuf::from(OsString::from_wide(&wide[VERBATIM_PREFIX.len()..]));
        }
    }
    path
}

fn paths_match_without_canonicalization(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn operation_error(kind: WorktreeOperationErrorKind, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(WorktreeOperationError {
        kind,
        message: message.into(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VerificationSummary {
    verification_success: bool,
    cleanup_success: bool,
    lease_recovery_required: bool,
    operation_success: bool,
}

fn verification_summary(
    status: WorktreeVerificationStatus,
    source_unchanged: bool,
    cleanup_success: bool,
) -> VerificationSummary {
    let verification_success = status == WorktreeVerificationStatus::Passed && source_unchanged;
    VerificationSummary {
        verification_success,
        cleanup_success,
        lease_recovery_required: !cleanup_success,
        operation_success: verification_success && cleanup_success,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_disposable_worktree_in, cleanup_lease, list_disposable_worktrees_in,
        validate_options, validate_run_id, verification_summary, write_lease, WorktreeLease,
        WorktreeStorage, WorktreeVerificationOptions, WorktreeVerificationStatus,
        WORKTREE_LEASE_SCHEMA_VERSION,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn run_ids_reject_path_traversal() {
        assert!(validate_run_id("verify-123_abc").is_ok());
        assert!(validate_run_id("../escape").is_err());
        assert!(validate_run_id("C:\\escape").is_err());
        assert!(validate_run_id("").is_err());
    }

    #[test]
    fn verification_options_reject_unbounded_process_limits() {
        let options = WorktreeVerificationOptions {
            git_timeout: std::time::Duration::from_secs(3_601),
            ..WorktreeVerificationOptions::default()
        };
        assert!(validate_options(options).is_err());

        let options = WorktreeVerificationOptions {
            output_limit_bytes: 16 * 1024 * 1024 + 1,
            ..WorktreeVerificationOptions::default()
        };
        assert!(validate_options(options).is_err());
    }

    #[test]
    fn cleanup_failure_does_not_mask_successful_verification() {
        let summary = verification_summary(WorktreeVerificationStatus::Passed, true, false);

        assert!(summary.verification_success);
        assert!(!summary.cleanup_success);
        assert!(summary.lease_recovery_required);
        assert!(!summary.operation_success);
    }

    #[test]
    fn empty_unregistered_lease_can_be_recovered_without_recursive_delete() {
        let root = unique_temp_dir("opticcode-worktree-storage");
        fs::create_dir(&root).expect("temporary root should be created");
        let storage = WorktreeStorage::prepare(&root.join("storage"), None)
            .expect("storage should be prepared");
        let source = root.join("source");
        fs::create_dir(&source).expect("source should be created");
        run_git(&source, &["init", "--quiet"]);
        fs::write(source.join("tracked.txt"), "tracked\n").expect("fixture should be written");
        run_git(&source, &["add", "--all"]);
        run_git(
            &source,
            &[
                "-c",
                "user.name=OpticCode Test",
                "-c",
                "user.email=opticcode-test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        );
        let source = fs::canonicalize(source).expect("source should resolve");
        let run_id = "verify-recovery-test";
        let target = storage.worktree_path(run_id);
        fs::create_dir(&target).expect("empty target should be reserved");
        let lease = WorktreeLease {
            schema_version: WORKTREE_LEASE_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            process_id: std::process::id(),
            created_unix_ms: 1,
            source_git_root: source.clone(),
            source_project: source,
            source_commit: "0".repeat(40),
            worktree_path: target.clone(),
        };
        write_lease(&storage, &lease).expect("lease should be written");

        let listed = list_disposable_worktrees_in(&storage).expect("leases should list");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].valid);
        assert_eq!(listed[0].registered, Some(false));

        let cleanup = cleanup_lease(
            &storage,
            &lease,
            std::time::Duration::from_secs(10),
            64 * 1024,
        )
        .expect("cleanup should run");
        assert!(cleanup.success);
        assert!(!target.exists());
        assert!(!storage.lease_path(run_id).exists());

        let repeated = cleanup_disposable_worktree_in(&storage, run_id)
            .expect("repeated cleanup should be idempotent");
        assert!(repeated.success);
        assert!(repeated.already_cleaned);
        assert!(!repeated.attempted);

        let _ = fs::remove_dir_all(root);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{stamp}", std::process::id()))
    }

    fn run_git(root: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("Git should start");
        assert!(
            output.status.success(),
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
