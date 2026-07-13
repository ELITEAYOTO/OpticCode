//! End-to-end verification of AST-ranged Java edits in a disposable Git worktree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

use crate::apply_transaction::{
    execute_apply_transaction, ApplyGitPolicy, ApplyTransactionRequest, FileMutation,
};
use crate::build_unified_diff;
use crate::git_state::{capture_git_state, GitChangeKind};
use crate::java_edits::{
    materialize_java_edits, propose_java_edits, JavaEditCounts, JavaEditFileValidation,
    JavaEditLimits, JavaEditMaterializedFile, JavaEditOptions, JavaEditProposal,
    JavaEditProposalReport, JavaEditRejection,
};
use crate::java_index::{JavaIndexCounts, JavaIndexSourceSummary, JavaIndexTruncation};
use crate::java_syntax::analyze_java_source;
use crate::process_runner::CancellationToken;
use crate::worktree::{
    verify_in_disposable_worktree_with_apply, worktree_operation_error_kind, WorktreeApplyReport,
    WorktreeOperationErrorKind, WorktreeVerificationOptions, WorktreeVerificationReport,
    WorktreeVerificationStatus,
};

pub const JAVA_EDIT_WORKTREE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaEditWorktreeErrorKind {
    Precondition,
    Verification,
    Git,
    Storage,
}

#[derive(Debug)]
struct JavaEditWorktreeError {
    kind: JavaEditWorktreeErrorKind,
    message: String,
}

impl std::fmt::Display for JavaEditWorktreeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JavaEditWorktreeError {}

pub fn java_edit_worktree_error_kind(error: &anyhow::Error) -> Option<JavaEditWorktreeErrorKind> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<JavaEditWorktreeError>())
        .map(|error| error.kind)
        .or_else(|| {
            worktree_operation_error_kind(error).map(|kind| match kind {
                WorktreeOperationErrorKind::Precondition
                | WorktreeOperationErrorKind::InvalidRunId => {
                    JavaEditWorktreeErrorKind::Precondition
                }
                WorktreeOperationErrorKind::Git => JavaEditWorktreeErrorKind::Git,
                WorktreeOperationErrorKind::Storage => JavaEditWorktreeErrorKind::Storage,
            })
        })
}

#[derive(Debug, Clone, Copy, Default)]
pub struct JavaEditWorktreeOptions {
    pub edits: JavaEditOptions,
    pub worktree: WorktreeVerificationOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaEditWorktreeStatus {
    Passed,
    NoChanges,
    SourceAnalysisFailed,
    SetupFailed,
    RevalidationFailed,
    MaterializationFailed,
    ApplyFailed,
    PostWriteValidationFailed,
    BuildFailed,
    FinalGitValidationFailed,
    Cancelled,
    VerificationFailed,
    SourceChanged,
}

impl JavaEditWorktreeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::NoChanges => "no_changes",
            Self::SourceAnalysisFailed => "source_analysis_failed",
            Self::SetupFailed => "setup_failed",
            Self::RevalidationFailed => "revalidation_failed",
            Self::MaterializationFailed => "materialization_failed",
            Self::ApplyFailed => "apply_failed",
            Self::PostWriteValidationFailed => "post_write_validation_failed",
            Self::BuildFailed => "build_failed",
            Self::FinalGitValidationFailed => "final_git_validation_failed",
            Self::Cancelled => "cancelled",
            Self::VerificationFailed => "verification_failed",
            Self::SourceChanged => "source_changed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditSourceAnalysisReport {
    pub root: PathBuf,
    pub input: PathBuf,
    pub analysis_complete: bool,
    pub safe_to_apply: bool,
    pub truncated: bool,
    pub proposals: usize,
    pub files_with_proposals: usize,
    pub rejections: usize,
    pub contract_fingerprint: String,
}

impl JavaEditSourceAnalysisReport {
    fn from_report(report: &JavaEditProposalReport, contract_fingerprint: String) -> Self {
        Self {
            root: report.root.clone(),
            input: report.input.clone(),
            analysis_complete: report.analysis_complete,
            safe_to_apply: report.safe_to_apply,
            truncated: report.truncated,
            proposals: report.proposals.len(),
            files_with_proposals: report.file_validations.len(),
            rejections: report.counts.rejections,
            contract_fingerprint,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditRevalidationReport {
    pub attempted: bool,
    pub success: bool,
    pub received: usize,
    pub valid: usize,
    pub refused: usize,
    pub source_contract_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_contract_fingerprint: Option<String>,
    pub worktree_analysis_complete: Option<bool>,
    pub worktree_safe_to_apply: Option<bool>,
    pub worktree_truncated: Option<bool>,
    pub errors: Vec<String>,
}

impl JavaEditRevalidationReport {
    fn pending(source_contract_fingerprint: String, received: usize) -> Self {
        Self {
            attempted: false,
            success: false,
            received,
            valid: 0,
            refused: 0,
            source_contract_fingerprint,
            worktree_contract_fingerprint: None,
            worktree_analysis_complete: None,
            worktree_safe_to_apply: None,
            worktree_truncated: None,
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditMaterializationReport {
    pub attempted: bool,
    pub success: bool,
    pub received: usize,
    pub valid: usize,
    pub refused: usize,
    pub files: Vec<JavaEditFileValidation>,
    pub errors: Vec<String>,
}

impl JavaEditMaterializationReport {
    fn pending(received: usize) -> Self {
        Self {
            attempted: false,
            success: false,
            received,
            valid: 0,
            refused: 0,
            files: Vec::new(),
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditPostWriteFileReport {
    pub path: PathBuf,
    pub expected_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_hash: Option<String>,
    pub expected_bytes: usize,
    pub actual_bytes: Option<usize>,
    pub bytes_match: bool,
    pub syntax_valid: bool,
    pub diagnostics: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditPostWriteReport {
    pub attempted: bool,
    pub success: bool,
    pub files_checked: usize,
    pub diagnostics: usize,
    pub files: Vec<JavaEditPostWriteFileReport>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaEditFinalGitReport {
    pub attempted: bool,
    pub success: bool,
    pub expected_changes: usize,
    pub matched_changes: usize,
    pub unexpected_changes: Vec<String>,
    pub errors: Vec<String>,
}

impl JavaEditFinalGitReport {
    fn pending(expected_changes: usize) -> Self {
        Self {
            attempted: false,
            success: false,
            expected_changes,
            matched_changes: 0,
            unexpected_changes: Vec::new(),
            errors: Vec::new(),
        }
    }
}

impl JavaEditPostWriteReport {
    fn pending() -> Self {
        Self {
            attempted: false,
            success: false,
            files_checked: 0,
            diagnostics: 0,
            files: Vec::new(),
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct JavaEditWorktreeReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub operation_success: bool,
    pub verification_success: bool,
    pub cleanup_success: bool,
    pub lease_recovery_required: bool,
    pub status: JavaEditWorktreeStatus,
    pub duration_ms: u64,
    pub source_analysis: JavaEditSourceAnalysisReport,
    pub revalidation: JavaEditRevalidationReport,
    pub materialization: JavaEditMaterializationReport,
    pub post_write_validation: JavaEditPostWriteReport,
    pub final_git_validation: JavaEditFinalGitReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeVerificationReport>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl JavaEditWorktreeReport {
    pub fn success(&self) -> bool {
        self.operation_success
    }

    pub fn to_display_string(&self) -> String {
        let mut output = String::new();
        output.push_str("Verified Java edits in disposable worktree:\n");
        output.push_str(&format!("- status: {}\n", self.status.as_str()));
        output.push_str(&format!(
            "- source proposals: {} in {} file(s)\n",
            self.source_analysis.proposals, self.source_analysis.files_with_proposals
        ));
        output.push_str(&format!(
            "- exact revalidation: {}\n",
            self.revalidation.success
        ));
        output.push_str(&format!(
            "- materialization: {}/{} valid\n",
            self.materialization.valid, self.materialization.received
        ));
        output.push_str(&format!(
            "- post-write syntax: {}\n",
            self.post_write_validation.success
        ));
        output.push_str(&format!(
            "- final Git hashes: {}/{}\n",
            self.final_git_validation.matched_changes, self.final_git_validation.expected_changes
        ));
        if let Some(worktree) = &self.worktree {
            output.push_str(&format!("- run id: {}\n", worktree.run_id));
            output.push_str(&format!(
                "- source unchanged: {}\n",
                worktree.source.unchanged
            ));
            output.push_str(&format!(
                "- build: {}\n",
                worktree.build.as_ref().is_some_and(|build| build.success)
            ));
        }
        output.push_str(&format!("- cleanup: {}\n", self.cleanup_success));
        output.push_str(&format!(
            "- duration: {:.3}s\n",
            self.duration_ms as f64 / 1_000.0
        ));
        for error in &self.errors {
            output.push_str(&format!("Error: {error}\n"));
        }
        for warning in &self.warnings {
            output.push_str(&format!("Warning: {warning}\n"));
        }
        output
    }
}

struct PipelineReports {
    revalidation: JavaEditRevalidationReport,
    materialization: JavaEditMaterializationReport,
    post_write: JavaEditPostWriteReport,
    apply_errors: Vec<String>,
}

pub fn verify_java_edits_in_worktree(
    source_project: &Path,
    options: JavaEditWorktreeOptions,
    cancellation: &CancellationToken,
) -> Result<JavaEditWorktreeReport> {
    verify_java_edits_in_worktree_with_source_hook(source_project, options, cancellation, || Ok(()))
}

fn verify_java_edits_in_worktree_with_source_hook<F>(
    source_project: &Path,
    options: JavaEditWorktreeOptions,
    cancellation: &CancellationToken,
    mut source_hook: F,
) -> Result<JavaEditWorktreeReport>
where
    F: FnMut() -> Result<()>,
{
    let started_at = Instant::now();
    let source_proposals = propose_java_edits(source_project, options.edits).map_err(|error| {
        java_edit_worktree_error(
            JavaEditWorktreeErrorKind::Precondition,
            format!("source Java edit analysis failed: {error:#}"),
        )
    })?;
    let source_fingerprint = proposal_contract_fingerprint(&source_proposals).map_err(|error| {
        java_edit_worktree_error(
            JavaEditWorktreeErrorKind::Verification,
            format!("source Java edit contract fingerprint failed: {error:#}"),
        )
    })?;
    let source_analysis =
        JavaEditSourceAnalysisReport::from_report(&source_proposals, source_fingerprint.clone());
    let received = source_proposals.proposals.len();
    let mut warnings = source_proposals.warnings.clone();

    if !source_proposals.analysis_complete
        || !source_proposals.safe_to_apply
        || source_proposals.truncated
    {
        return Ok(JavaEditWorktreeReport {
            schema_version: JAVA_EDIT_WORKTREE_SCHEMA_VERSION,
            operation: "java_edits_verify",
            operation_success: false,
            verification_success: false,
            cleanup_success: true,
            lease_recovery_required: false,
            status: JavaEditWorktreeStatus::SourceAnalysisFailed,
            duration_ms: duration_ms(started_at),
            source_analysis,
            revalidation: JavaEditRevalidationReport::pending(source_fingerprint, received),
            materialization: JavaEditMaterializationReport::pending(received),
            post_write_validation: JavaEditPostWriteReport::pending(),
            final_git_validation: JavaEditFinalGitReport::pending(0),
            worktree: None,
            errors: vec![
                "source Java edit analysis is incomplete, unsafe, or truncated; no worktree was created"
                    .to_string(),
            ],
            warnings,
        });
    }

    if source_proposals.proposals.is_empty() {
        let source_state = capture_git_state(source_project).map_err(|error| {
            java_edit_worktree_error(
                JavaEditWorktreeErrorKind::Precondition,
                format!("source project must be a Git worktree: {error:#}"),
            )
        })?;
        if !source_state.changes.is_empty() {
            return Err(java_edit_worktree_error(
                JavaEditWorktreeErrorKind::Precondition,
                format!(
                    "source Git worktree must be clean; found {} change(s)",
                    source_state.changes.len()
                ),
            ));
        }
        return Ok(JavaEditWorktreeReport {
            schema_version: JAVA_EDIT_WORKTREE_SCHEMA_VERSION,
            operation: "java_edits_verify",
            operation_success: true,
            verification_success: true,
            cleanup_success: true,
            lease_recovery_required: false,
            status: JavaEditWorktreeStatus::NoChanges,
            duration_ms: duration_ms(started_at),
            source_analysis,
            revalidation: JavaEditRevalidationReport::pending(source_fingerprint, 0),
            materialization: JavaEditMaterializationReport::pending(0),
            post_write_validation: JavaEditPostWriteReport::pending(),
            final_git_validation: JavaEditFinalGitReport::pending(0),
            worktree: None,
            errors: Vec::new(),
            warnings,
        });
    }

    source_hook().map_err(|error| {
        java_edit_worktree_error(
            JavaEditWorktreeErrorKind::Precondition,
            format!("source changed during Java edit verification setup: {error:#}"),
        )
    })?;

    let source_root = source_proposals.root.clone();
    let mut pipeline = PipelineReports {
        revalidation: JavaEditRevalidationReport::pending(source_fingerprint.clone(), received),
        materialization: JavaEditMaterializationReport::pending(received),
        post_write: JavaEditPostWriteReport::pending(),
        apply_errors: Vec::new(),
    };

    let worktree = verify_in_disposable_worktree_with_apply(
        source_project,
        options.worktree,
        cancellation,
        |worktree_project| {
            pipeline.revalidation.attempted = true;
            let worktree_proposals = match propose_java_edits(worktree_project, options.edits) {
                Ok(report) => report,
                Err(error) => {
                    let message = format!("worktree Java edit analysis failed: {error:#}");
                    pipeline.revalidation.refused = received;
                    pipeline.revalidation.errors.push(message.clone());
                    return Err(anyhow!(message));
                }
            };
            let worktree_fingerprint = match proposal_contract_fingerprint(&worktree_proposals) {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    let message = format!("worktree proposal fingerprint failed: {error:#}");
                    pipeline.revalidation.refused = received;
                    pipeline.revalidation.errors.push(message.clone());
                    return Err(anyhow!(message));
                }
            };
            pipeline.revalidation.worktree_contract_fingerprint =
                Some(worktree_fingerprint.clone());
            pipeline.revalidation.worktree_analysis_complete =
                Some(worktree_proposals.analysis_complete);
            pipeline.revalidation.worktree_safe_to_apply = Some(worktree_proposals.safe_to_apply);
            pipeline.revalidation.worktree_truncated = Some(worktree_proposals.truncated);

            let contract_matches = worktree_proposals.analysis_complete
                && worktree_proposals.safe_to_apply
                && !worktree_proposals.truncated
                && worktree_proposals.proposals.len() == received
                && worktree_fingerprint == source_fingerprint;
            if !contract_matches {
                let message = format!(
                    "source/worktree Java edit contract mismatch: source={}, worktree={}, source proposals={}, worktree proposals={}",
                    source_fingerprint,
                    worktree_fingerprint,
                    received,
                    worktree_proposals.proposals.len()
                );
                pipeline.revalidation.refused = received;
                pipeline.revalidation.errors.push(message.clone());
                return Err(anyhow!(message));
            }
            pipeline.revalidation.success = true;
            pipeline.revalidation.valid = received;

            pipeline.materialization.attempted = true;
            let materialized = match materialize_java_edits(worktree_project, &worktree_proposals) {
                Ok(files) => files,
                Err(error) => {
                    let message = format!("worktree Java edit materialization failed: {error:#}");
                    pipeline.materialization.refused = received;
                    pipeline.materialization.errors.push(message.clone());
                    return Err(anyhow!(message));
                }
            };
            let valid = materialized
                .iter()
                .map(|file| file.validation.edit_count)
                .sum::<usize>();
            if valid != received {
                let message = format!(
                    "materialized proposal count mismatch: expected {received}, found {valid}"
                );
                pipeline.materialization.refused = received;
                pipeline.materialization.errors.push(message.clone());
                return Err(anyhow!(message));
            }
            pipeline.materialization.success = true;
            pipeline.materialization.valid = valid;
            pipeline.materialization.files = materialized
                .iter()
                .map(|file| file.validation.clone())
                .collect();

            let patch = build_materialized_patch(&materialized)?;
            let mutations = materialized
                .iter()
                .map(|file| {
                    FileMutation::replace(
                        file.path.clone(),
                        file.expected_before.clone(),
                        file.desired_after.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let request = ApplyTransactionRequest::new(
                worktree_project,
                patch.as_bytes().to_vec(),
                mutations,
            )
            .with_git_policy(ApplyGitPolicy::RequireClean)
            .with_copied_from(&source_root);
            let transaction = match execute_apply_transaction(request) {
                Ok(result) => result,
                Err(error) => {
                    let message = format!("transactional Java edit apply failed: {error:#}");
                    pipeline.apply_errors.push(message.clone());
                    return Err(anyhow!(message));
                }
            };
            let committed = transaction.committed();
            if committed {
                pipeline.post_write = validate_post_write(worktree_project, &materialized);
            }
            let apply_success = committed && pipeline.post_write.success;
            let patch_bytes = patch.len();
            let patch_complete = patch_bytes <= options.worktree.output_limit_bytes;
            let report_patch = if patch_complete {
                patch.clone()
            } else {
                String::new()
            };

            Ok(WorktreeApplyReport {
                success: apply_success,
                change_count: materialized.len(),
                files: materialized
                    .iter()
                    .map(|file| normalized_path(&file.path))
                    .collect(),
                patch: report_patch,
                patch_bytes,
                patch_hash: content_hash(patch.as_bytes()),
                patch_complete,
                check: None,
                apply: None,
                transaction: Some(transaction),
            })
        },
    )?;

    if worktree
        .apply
        .as_ref()
        .is_some_and(|apply| !apply.patch_complete)
    {
        warnings.push(
            "transaction patch content exceeded the output limit and was omitted; patch_hash, final Git diff and Git state remain available"
                .to_string(),
        );
    }

    let final_git_validation = if pipeline.post_write.success {
        validate_final_git_state(&worktree, &pipeline.materialization.files)
    } else {
        JavaEditFinalGitReport::pending(pipeline.materialization.files.len())
    };
    let status = derive_status(&pipeline, &final_git_validation, &worktree);
    let semantic_success = pipeline.revalidation.success
        && pipeline.materialization.success
        && pipeline.post_write.success
        && final_git_validation.success;
    let verification_success = semantic_success && worktree.verification_success;
    let cleanup_success = worktree.cleanup_success;
    let mut errors = pipeline.revalidation.errors.clone();
    errors.extend(pipeline.materialization.errors.iter().cloned());
    errors.extend(pipeline.post_write.errors.iter().cloned());
    errors.extend(final_git_validation.errors.iter().cloned());
    errors.extend(pipeline.apply_errors.iter().cloned());

    Ok(JavaEditWorktreeReport {
        schema_version: JAVA_EDIT_WORKTREE_SCHEMA_VERSION,
        operation: "java_edits_verify",
        operation_success: verification_success && cleanup_success,
        verification_success,
        cleanup_success,
        lease_recovery_required: worktree.lease_recovery_required,
        status,
        duration_ms: duration_ms(started_at),
        source_analysis,
        revalidation: pipeline.revalidation,
        materialization: pipeline.materialization,
        post_write_validation: pipeline.post_write,
        final_git_validation,
        worktree: Some(worktree),
        errors,
        warnings,
    })
}

fn derive_status(
    pipeline: &PipelineReports,
    final_git_validation: &JavaEditFinalGitReport,
    worktree: &WorktreeVerificationReport,
) -> JavaEditWorktreeStatus {
    if pipeline.revalidation.attempted && !pipeline.revalidation.success {
        return JavaEditWorktreeStatus::RevalidationFailed;
    }
    if pipeline.materialization.attempted && !pipeline.materialization.success {
        return JavaEditWorktreeStatus::MaterializationFailed;
    }
    if pipeline.post_write.attempted && !pipeline.post_write.success {
        return JavaEditWorktreeStatus::PostWriteValidationFailed;
    }
    if final_git_validation.attempted && !final_git_validation.success {
        return JavaEditWorktreeStatus::FinalGitValidationFailed;
    }
    match worktree.status {
        WorktreeVerificationStatus::Passed => JavaEditWorktreeStatus::Passed,
        WorktreeVerificationStatus::SetupFailed => JavaEditWorktreeStatus::SetupFailed,
        WorktreeVerificationStatus::ApplyFailed => JavaEditWorktreeStatus::ApplyFailed,
        WorktreeVerificationStatus::BuildFailed => JavaEditWorktreeStatus::BuildFailed,
        WorktreeVerificationStatus::Cancelled => JavaEditWorktreeStatus::Cancelled,
        WorktreeVerificationStatus::VerificationFailed => {
            JavaEditWorktreeStatus::VerificationFailed
        }
        WorktreeVerificationStatus::SourceChanged => JavaEditWorktreeStatus::SourceChanged,
    }
}

fn validate_final_git_state(
    worktree: &WorktreeVerificationReport,
    files: &[JavaEditFileValidation],
) -> JavaEditFinalGitReport {
    let mut report = JavaEditFinalGitReport {
        attempted: true,
        success: false,
        expected_changes: files.len(),
        matched_changes: 0,
        unexpected_changes: Vec::new(),
        errors: Vec::new(),
    };
    let Some(snapshot) = worktree.worktree_after.as_ref() else {
        report
            .errors
            .push("final worktree Git snapshot is unavailable".to_string());
        return report;
    };

    let project_prefix = &worktree.source.relative_project;
    let mut expected = files
        .iter()
        .map(|file| {
            (
                normalized_path(&project_prefix.join(&file.path)),
                file.proposed_hash.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let metadata_prefix = normalized_path(&project_prefix.join(".opticcode"));

    for change in &snapshot.changes {
        if let Some(expected_hash) = expected.remove(&change.path) {
            let kind_matches = change.kind == GitChangeKind::Modified;
            let hash_matches = change.content_fingerprint.as_deref() == Some(expected_hash);
            if kind_matches && hash_matches {
                report.matched_changes += 1;
            } else {
                report.errors.push(format!(
                    "final Git state mismatch for {}: kind={:?}, expected hash={}, actual hash={}",
                    change.path,
                    change.kind,
                    expected_hash,
                    change.content_fingerprint.as_deref().unwrap_or("missing")
                ));
            }
            continue;
        }

        if change.path == metadata_prefix
            || change
                .path
                .strip_prefix(&metadata_prefix)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            continue;
        }
        report.unexpected_changes.push(change.path.clone());
    }

    for (path, expected_hash) in expected {
        report.errors.push(format!(
            "expected Java edit is absent from final Git state: {path} ({expected_hash})"
        ));
    }
    if !report.unexpected_changes.is_empty() {
        report.errors.push(format!(
            "final Git state contains {} unexpected change(s)",
            report.unexpected_changes.len()
        ));
    }
    report.success = report.errors.is_empty()
        && report.unexpected_changes.is_empty()
        && report.matched_changes == report.expected_changes;
    report
}

fn build_materialized_patch(files: &[JavaEditMaterializedFile]) -> Result<String> {
    let mut patch = String::new();
    for file in files {
        let original = std::str::from_utf8(&file.expected_before).with_context(|| {
            format!("materialized source is not UTF-8: {}", file.path.display())
        })?;
        let proposed = std::str::from_utf8(&file.desired_after).with_context(|| {
            format!(
                "materialized replacement is not UTF-8: {}",
                file.path.display()
            )
        })?;
        if !patch.is_empty() {
            patch.push('\n');
        }
        patch.push_str(&build_unified_diff(
            &file.path,
            original,
            proposed,
            "Tree-sitter exact Java legacy edits",
        ));
    }
    if patch.is_empty() {
        bail!("Java edit materialization unexpectedly produced an empty patch");
    }
    Ok(patch)
}

fn validate_post_write(
    worktree_project: &Path,
    files: &[JavaEditMaterializedFile],
) -> JavaEditPostWriteReport {
    let mut report = JavaEditPostWriteReport {
        attempted: true,
        success: true,
        files_checked: 0,
        diagnostics: 0,
        files: Vec::with_capacity(files.len()),
        errors: Vec::new(),
    };

    for file in files {
        report.files_checked += 1;
        let absolute = worktree_project.join(&file.path);
        let bytes = match fs::read(&absolute) {
            Ok(bytes) => bytes,
            Err(error) => {
                report.success = false;
                report.errors.push(format!(
                    "failed to read post-write Java source {}: {error}",
                    file.path.display()
                ));
                report.files.push(JavaEditPostWriteFileReport {
                    path: file.path.clone(),
                    expected_hash: file.validation.proposed_hash.clone(),
                    actual_hash: None,
                    expected_bytes: file.desired_after.len(),
                    actual_bytes: None,
                    bytes_match: false,
                    syntax_valid: false,
                    diagnostics: 0,
                });
                continue;
            }
        };
        let actual_hash = content_hash(&bytes);
        let bytes_match = bytes == file.desired_after
            && actual_hash == file.validation.proposed_hash
            && bytes.len() == file.validation.proposed_bytes;
        let (syntax_valid, diagnostics) = match std::str::from_utf8(&bytes) {
            Ok(source) => match analyze_java_source(file.path.clone(), source) {
                Ok(parsed) => (parsed.syntax_valid, parsed.diagnostics.len()),
                Err(error) => {
                    report.errors.push(format!(
                        "failed to reparse post-write Java source {}: {error:#}",
                        file.path.display()
                    ));
                    (false, 0)
                }
            },
            Err(_) => {
                report.errors.push(format!(
                    "post-write Java source is not UTF-8: {}",
                    file.path.display()
                ));
                (false, 0)
            }
        };
        report.diagnostics = report.diagnostics.saturating_add(diagnostics);
        if !bytes_match {
            report.errors.push(format!(
                "post-write bytes differ from the materialized edit for {}",
                file.path.display()
            ));
        }
        if !syntax_valid {
            report.errors.push(format!(
                "post-write Java syntax is invalid for {} ({diagnostics} diagnostic(s))",
                file.path.display()
            ));
        }
        report.success &= bytes_match && syntax_valid;
        report.files.push(JavaEditPostWriteFileReport {
            path: file.path.clone(),
            expected_hash: file.validation.proposed_hash.clone(),
            actual_hash: Some(actual_hash),
            expected_bytes: file.desired_after.len(),
            actual_bytes: Some(bytes.len()),
            bytes_match,
            syntax_valid,
            diagnostics,
        });
    }
    report.success &= report.files_checked == files.len() && !files.is_empty();
    report
}

#[derive(Serialize)]
struct JavaEditProposalContract<'a> {
    schema_version: u32,
    rule_set: &'a str,
    index_schema_version: u32,
    index_source: &'a JavaIndexSourceSummary,
    index_truncation: &'a JavaIndexTruncation,
    index_counts: &'a JavaIndexCounts,
    limits: &'a JavaEditLimits,
    index_analysis_complete: bool,
    analysis_complete: bool,
    safe_to_apply: bool,
    truncated: bool,
    proposals_truncated: bool,
    rejections_truncated: bool,
    counts: &'a JavaEditCounts,
    proposals: &'a [JavaEditProposal],
    file_validations: &'a [JavaEditFileValidation],
    rejections: &'a [JavaEditRejection],
}

fn proposal_contract_fingerprint(report: &JavaEditProposalReport) -> Result<String> {
    let contract = JavaEditProposalContract {
        schema_version: report.schema_version,
        rule_set: report.rule_set,
        index_schema_version: report.index_schema_version,
        index_source: &report.index_source,
        index_truncation: &report.index_truncation,
        index_counts: &report.index_counts,
        limits: &report.limits,
        index_analysis_complete: report.index_analysis_complete,
        analysis_complete: report.analysis_complete,
        safe_to_apply: report.safe_to_apply,
        truncated: report.truncated,
        proposals_truncated: report.proposals_truncated,
        rejections_truncated: report.rejections_truncated,
        counts: &report.counts,
        proposals: &report.proposals,
        file_validations: &report.file_validations,
        rejections: &report.rejections,
    };
    let bytes = serde_json::to_vec(&contract).context("failed to serialize Java edit contract")?;
    Ok(content_hash(&bytes))
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn content_hash(bytes: &[u8]) -> String {
    format!("blake3:{}:{}", bytes.len(), blake3::hash(bytes))
}

fn duration_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn java_edit_worktree_error(
    kind: JavaEditWorktreeErrorKind,
    message: impl Into<String>,
) -> anyhow::Error {
    anyhow::Error::new(JavaEditWorktreeError {
        kind,
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        proposal_contract_fingerprint, verify_java_edits_in_worktree_with_source_hook,
        JavaEditWorktreeOptions, JavaEditWorktreeStatus,
    };
    use crate::java_edits::{materialize_java_edits, propose_java_edits, JavaEditOptions};
    use crate::process_runner::CancellationToken;

    #[test]
    fn proposal_contract_is_root_independent_and_content_sensitive() {
        let first = temporary_root("contract-first");
        let second = temporary_root("contract-second");
        write_java(&first, "GUNPOWDER");
        write_java(&second, "GUNPOWDER");

        let first_report = propose_java_edits(&first, JavaEditOptions::default()).unwrap();
        let second_report = propose_java_edits(&second, JavaEditOptions::default()).unwrap();
        assert_eq!(first_report.proposals.len(), 1);
        assert_eq!(second_report.proposals.len(), 1);
        assert_eq!(
            proposal_contract_fingerprint(&first_report).unwrap(),
            proposal_contract_fingerprint(&second_report).unwrap()
        );

        write_java(&second, "WOODEN_SHOVEL");
        let changed_report = propose_java_edits(&second, JavaEditOptions::default()).unwrap();
        assert_eq!(changed_report.proposals.len(), 1);
        assert_ne!(
            proposal_contract_fingerprint(&first_report).unwrap(),
            proposal_contract_fingerprint(&changed_report).unwrap()
        );

        remove_temporary_root(&first);
        remove_temporary_root(&second);
    }

    #[test]
    fn materialization_rejects_source_drift_after_proposal() {
        let root = temporary_root("materialization-drift");
        write_java(&root, "GUNPOWDER");
        let report = propose_java_edits(&root, JavaEditOptions::default()).unwrap();
        assert!(report.safe_to_apply);
        assert_eq!(report.proposals.len(), 1);

        let path = root.join("Plugin.java");
        let mut changed = fs::read_to_string(&path).unwrap();
        changed.push_str("// concurrent change\n");
        fs::write(&path, changed).unwrap();
        let error = materialize_java_edits(&root, &report).unwrap_err();
        assert!(format!("{error:#}").contains("changed after indexing"));

        remove_temporary_root(&root);
    }

    #[test]
    fn worktree_revalidation_refuses_a_new_head_with_a_different_contract() {
        let root = temporary_root("contract-drift");
        write_java(&root, "GUNPOWDER");
        fs::write(root.join(".gitignore"), ".opticcode/\ntarget/\n").unwrap();
        run_git(&root, &["init", "--quiet"]);
        run_git(&root, &["config", "core.autocrlf", "false"]);
        run_git(&root, &["add", "--all"]);
        commit(&root, "initial contract");

        let report = verify_java_edits_in_worktree_with_source_hook(
            &root,
            JavaEditWorktreeOptions::default(),
            &CancellationToken::new(),
            || {
                write_java(&root, "WOODEN_SHOVEL");
                run_git(&root, &["add", "--all"]);
                commit(&root, "changed contract");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(report.status, JavaEditWorktreeStatus::RevalidationFailed);
        assert!(!report.operation_success);
        assert!(report.revalidation.attempted);
        assert!(!report.revalidation.success);
        assert_eq!(report.revalidation.received, 1);
        assert_eq!(report.revalidation.refused, 1);
        let worktree = report.worktree.unwrap();
        assert!(worktree.source.unchanged);
        assert!(worktree.cleanup_success);
        assert!(!worktree.worktree_root.exists());

        remove_temporary_root(&root);
    }

    fn temporary_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-java-edit-worktree-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    fn write_java(root: &Path, member: &str) {
        fs::write(
            root.join("Plugin.java"),
            format!(
                "import org.bukkit.Material;\nclass Plugin {{ Object value = Material.{member}; }}\n"
            ),
        )
        .unwrap();
    }

    fn remove_temporary_root(root: &Path) {
        if root.starts_with(std::env::temp_dir()) {
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit(root: &Path, message: &str) {
        run_git(
            root,
            &[
                "-c",
                "user.name=OpticCode Test",
                "-c",
                "user.email=opticcode-test@example.invalid",
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                message,
            ],
        );
    }
}
