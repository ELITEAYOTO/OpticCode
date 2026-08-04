use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Result};
use opticcode_policy::{
    ApplyPatchAction, CleanupWorktreeAction, CreateWorktreeAction, GitReadAction, GitReadOperation,
    PathTarget, PolicyAction, PolicyEngine, PolicyMode,
};
use opticcode_tools::apply_transaction::{
    execute_apply_transaction, ApplyGitPolicy, ApplyTransactionRequest,
};
use opticcode_tools::process_runner::CancellationToken;
use opticcode_tools::worktree::{
    run_in_disposable_worktree, WorktreeOwner, WorktreeVerificationOptions,
};

use crate::build::{discover_offline_build, run_offline_build};
use crate::policy_adapter::{observe_repository, PolicySession};
use crate::transaction::{
    proposal_contract_bytes, proposal_files_hash, proposal_paths, reparse_java_files,
    verify_snapshots_at, verify_worktree_base_at, worktree_expected_hash, worktree_mutations,
};
use crate::{
    capture_verified_diff, content_hash, new_edit_id, unix_millis, ChatEditVerificationReport,
    EditProcessReport, EditRuntimeOptions, EditStageReport, EditStageStatus, PolicyDecisionRecord,
    ProposalFileStatus, ProposalRecord, ProposalState, ProposalStore, VerifiedDiff,
    CHAT_EDIT_REPORT_SCHEMA_VERSION,
};

#[derive(Debug)]
struct VerificationProgress {
    apply: EditStageReport,
    reparse: EditStageReport,
    build: EditStageReport,
    tests: EditStageReport,
    diff: EditStageReport,
    processes: Vec<EditProcessReport>,
    verified_diff: Option<VerifiedDiff>,
}

impl Default for VerificationProgress {
    fn default() -> Self {
        Self {
            apply: not_run("transactional worktree apply did not run"),
            reparse: not_run("Tree-sitter reparse did not run"),
            build: not_run("offline build did not run"),
            tests: not_run("offline tests did not run"),
            diff: not_run("verified Git diff was not captured"),
            processes: Vec::new(),
            verified_diff: None,
        }
    }
}

pub fn verify_edit_proposal(
    store: &ProposalStore,
    proposal_id: &str,
    policy: &PolicyEngine,
    options: &EditRuntimeOptions,
    cancellation: &CancellationToken,
) -> Result<ProposalRecord> {
    let record = store.expire_if_needed(proposal_id, unix_millis())?;
    validate_verification_preconditions(&record, options)?;
    let observed = observe_repository(options, cancellation)?;
    if !observed.clean
        || observed.boundary.head != record.plan.base_head
        || observed.working_tree_digest != record.plan.working_tree_digest
        || observed.root_hash != record.workspace_root_hash
    {
        bail!("workspace HEAD, digest, cleanliness, or identity drifted after plan validation");
    }
    let policy_session = PolicySession::new(policy, options, &observed);

    let decisions = RefCell::new(Vec::<PolicyDecisionRecord>::new());
    let progress = RefCell::new(VerificationProgress::default());
    let contract = proposal_contract_bytes(&record)?;
    let contract_hash = content_hash(&contract);
    let files_hash = proposal_files_hash(&record.files);
    let (existing_paths, created_paths) = proposal_paths(&record.files);
    let transaction_id = new_edit_id("worktree")?;
    let worktree_options = WorktreeVerificationOptions {
        build_timeout: options.build_timeout,
        git_timeout: options.git_timeout,
        output_limit_bytes: options.output_limit_bytes,
    };

    let run = run_in_disposable_worktree(
        &observed.root,
        WorktreeOwner::new(&options.workspace_id, &options.request_id)?,
        worktree_options,
        cancellation,
        |intent| {
            policy_session.authorize(
                PolicyMode::WorktreeEdit,
                PolicyAction::CreateWorktree(CreateWorktreeAction {
                    repository_root: observed.root.clone(),
                    destination: intent.destination.clone(),
                    base_head: intent.base_head.clone(),
                    run_id: intent.run_id.clone(),
                    detached: true,
                }),
                None,
                None,
                "worktree_create",
                &mut decisions.borrow_mut(),
            )
        },
        |context| {
            store.transition(
                proposal_id,
                ProposalState::WorktreePrepared,
                format!("disposable worktree {} prepared", context.run_id),
            )?;
            verify_worktree_base_at(&context.worktree_root, &record.files)?;

            for file in &record.files {
                let action = match file.status {
                    ProposalFileStatus::Modified => PolicyAction::WriteFile(PathTarget {
                        root: context.worktree_root.clone(),
                        path: PathBuf::from(&file.path),
                        range: None,
                        expected_hash: worktree_expected_hash(&context.worktree_root, file)?,
                    }),
                    ProposalFileStatus::Created => PolicyAction::CreateFile(PathTarget {
                        root: context.worktree_root.clone(),
                        path: PathBuf::from(&file.path),
                        range: None,
                        expected_hash: None,
                    }),
                };
                policy_session.authorize(
                    PolicyMode::WorktreeEdit,
                    action,
                    Some(context),
                    None,
                    "worktree_write",
                    &mut decisions.borrow_mut(),
                )?;
            }
            policy_session.authorize(
                PolicyMode::WorktreeEdit,
                PolicyAction::ApplyPatch(ApplyPatchAction {
                    root: context.worktree_root.clone(),
                    paths: existing_paths.clone(),
                    created_paths: created_paths.clone(),
                    diff_hash: contract_hash.clone(),
                    files_hash: files_hash.clone(),
                    transaction_id: transaction_id.clone(),
                    base_head: record.plan.base_head.clone(),
                }),
                Some(context),
                None,
                "worktree_apply",
                &mut decisions.borrow_mut(),
            )?;

            let apply = execute_apply_transaction(
                ApplyTransactionRequest::new(
                    &context.worktree_root,
                    contract.clone(),
                    worktree_mutations(&context.worktree_root, &record.files)?,
                )
                .with_git_policy(ApplyGitPolicy::RequireClean)
                .with_copied_from(&observed.root)
                .with_transaction_id(&transaction_id),
            )?;
            progress.borrow_mut().apply = if apply.committed() {
                EditStageReport::passed(
                    format!("APPLY-001 committed {} file(s)", apply.modified_files.len()),
                    apply.duration_ms,
                )
            } else {
                EditStageReport {
                    status: EditStageStatus::Failed,
                    duration_ms: apply.duration_ms,
                    summary: "APPLY-001 did not commit the worktree proposal".to_string(),
                    errors: apply.errors.clone(),
                }
            };
            if !apply.committed() {
                bail!("transactional worktree apply failed");
            }
            store.transition(
                proposal_id,
                ProposalState::WorktreeApplied,
                "validated snapshots applied transactionally in disposable worktree",
            )?;
            verify_snapshots_at(&context.worktree_root, &record.files, true)?;

            let reparse = reparse_java_files(&context.worktree_root, &record.files);
            let reparse_passed = reparse.status == EditStageStatus::Passed;
            progress.borrow_mut().reparse = reparse;
            if !reparse_passed {
                bail!("Tree-sitter rejected the proposed Java source");
            }

            let mut diff_paths = existing_paths.clone();
            diff_paths.extend(created_paths.iter().cloned());
            diff_paths.sort();
            policy_session.authorize(
                PolicyMode::WorktreeEdit,
                PolicyAction::GitRead(GitReadAction {
                    repository_root: context.worktree_root.clone(),
                    operation: GitReadOperation::Diff,
                    paths: diff_paths,
                }),
                Some(context),
                None,
                "worktree_diff",
                &mut decisions.borrow_mut(),
            )?;
            let diff_started = Instant::now();
            let diff = capture_verified_diff(
                &context.worktree_root,
                &record.files,
                options.git_timeout,
                cancellation,
            )?;
            enforce_actual_diff_limits(&record, &diff)?;
            progress.borrow_mut().diff = EditStageReport::passed(
                format!(
                    "verified {} file(s), +{} -{}, {} hunk(s)",
                    diff.statistics.files,
                    diff.statistics.additions,
                    diff.statistics.deletions,
                    diff.statistics.hunks
                ),
                elapsed_ms(diff_started),
            );
            progress.borrow_mut().verified_diff = Some(diff);

            store.transition(
                proposal_id,
                ProposalState::BuildRunning,
                "offline allowlisted build started in disposable worktree",
            )?;
            let invocation =
                discover_offline_build(&context.worktree_project, &record.plan.validations)?;
            policy_session.authorize(
                PolicyMode::WorktreeEdit,
                PolicyAction::RunProcess(
                    invocation.policy_action(&context.worktree_project, options),
                ),
                Some(context),
                None,
                "offline_build",
                &mut decisions.borrow_mut(),
            )?;
            let build = run_offline_build(
                &context.worktree_project,
                &invocation,
                &record.plan.validations,
                options,
                cancellation,
            )?;
            let build_passed = build.build.status == EditStageStatus::Passed
                && !matches!(
                    build.tests.status,
                    EditStageStatus::Failed | EditStageStatus::Cancelled
                );
            {
                let mut progress = progress.borrow_mut();
                progress.build = build.build;
                progress.tests = build.tests;
                progress.processes.push(build.process);
            }
            verify_snapshots_at(&context.worktree_root, &record.files, true)?;
            if !build_passed {
                bail!("offline build or required tests failed");
            }
            Ok(())
        },
        |context| {
            policy_session.authorize(
                PolicyMode::WorktreeEdit,
                PolicyAction::CleanupWorktree(CleanupWorktreeAction {
                    repository_root: observed.root.clone(),
                    worktree_root: context.worktree_root.clone(),
                    run_id: context.run_id.clone(),
                }),
                Some(context),
                None,
                "worktree_cleanup",
                &mut decisions.borrow_mut(),
            )
        },
    );

    let policy_records = decisions.into_inner();
    let progress = progress.into_inner();
    match run {
        Ok(run) => finish_verification(
            store,
            record,
            options,
            policy_records,
            progress,
            Some(run.context.run_id),
            run.operation_error,
            run.cleanup.success,
            run.source_unchanged,
            run.errors,
        ),
        Err(error) => finish_verification(
            store,
            record,
            options,
            policy_records,
            progress,
            None,
            Some(format!("{error:#}")),
            false,
            false,
            Vec::new(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_verification(
    store: &ProposalStore,
    record: ProposalRecord,
    options: &EditRuntimeOptions,
    policy: Vec<PolicyDecisionRecord>,
    progress: VerificationProgress,
    run_id: Option<String>,
    operation_error: Option<String>,
    cleanup_success: bool,
    source_unchanged: bool,
    mut errors: Vec<String>,
) -> Result<ProposalRecord> {
    if let Some(error) = operation_error {
        errors.push(error);
    }
    let success = errors.is_empty()
        && cleanup_success
        && source_unchanged
        && progress.apply.status == EditStageStatus::Passed
        && progress.reparse.status == EditStageStatus::Passed
        && progress.build.status == EditStageStatus::Passed
        && progress.diff.status == EditStageStatus::Passed;
    let cleanup = if cleanup_success {
        EditStageReport::passed("disposable worktree and lease removed", 0)
    } else {
        EditStageReport::failed(
            "disposable worktree cleanup requires recovery",
            0,
            "targeted cleanup did not complete",
        )
    };
    let worktree = if run_id.is_some() {
        EditStageReport::passed("detached owned worktree prepared", 0)
    } else {
        EditStageReport::failed(
            "disposable worktree setup failed",
            0,
            errors
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown setup failure".to_string()),
        )
    };
    let next = if success {
        ProposalState::Verified
    } else {
        ProposalState::VerificationFailed
    };
    store.transition(
        &record.proposal_id,
        next,
        if success {
            "worktree apply, reparse, offline build, diff, cleanup, and source guard passed"
        } else {
            "proposal verification failed closed"
        },
    )?;
    store.record_policy(&record.proposal_id, policy.clone())?;
    let report = ChatEditVerificationReport {
        schema_version: CHAT_EDIT_REPORT_SCHEMA_VERSION,
        request_id: options.request_id.clone(),
        proposal_id: record.proposal_id.clone(),
        base_head: record.plan.base_head.clone(),
        working_tree_digest: record.plan.working_tree_digest.clone(),
        worktree_run_id: run_id,
        success,
        source_unchanged,
        lease_recovery_required: !cleanup_success,
        worktree,
        apply: progress.apply,
        reparse: progress.reparse,
        build: progress.build,
        tests: progress.tests,
        diff: progress.diff,
        cleanup,
        processes: progress.processes,
        policy,
        verified_at_unix_ms: success.then(unix_millis),
        warnings: Vec::new(),
        errors,
    };
    store.record_verification(
        &record.proposal_id,
        report,
        success.then_some(progress.verified_diff).flatten(),
    )
}

fn validate_verification_preconditions(
    record: &ProposalRecord,
    options: &EditRuntimeOptions,
) -> Result<()> {
    if record.workspace_id != options.workspace_id || record.plan.profile != options.profile {
        bail!("proposal belongs to another workspace, request, or profile");
    }
    if !matches!(
        record.state,
        ProposalState::Validated | ProposalState::Verified | ProposalState::VerificationFailed
    ) {
        bail!("proposal state {:?} cannot be verified", record.state);
    }
    if record.expired(unix_millis()) {
        bail!("proposal expired before verification");
    }
    Ok(())
}

fn enforce_actual_diff_limits(record: &ProposalRecord, diff: &VerifiedDiff) -> Result<()> {
    let limits = record.plan.limits;
    if diff.statistics.files > limits.max_files
        || diff.statistics.added_files > limits.max_created_files
        || diff.statistics.hunks > limits.max_hunks
        || diff.statistics.additions > limits.max_added_lines
        || diff.statistics.deletions > limits.max_deleted_lines
        || diff
            .statistics
            .additions
            .saturating_add(diff.statistics.deletions)
            > limits.max_changed_lines
        || diff.statistics.patch_bytes > limits.max_proposal_bytes
        || diff.statistics.deleted_files != 0
        || diff.statistics.renamed_files != 0
        || diff.statistics.binary_files != 0
    {
        bail!("actual Git diff exceeds the immutable EditPlan limits");
    }
    Ok(())
}

fn not_run(summary: &str) -> EditStageReport {
    EditStageReport {
        status: EditStageStatus::NotRun,
        duration_ms: 0,
        summary: summary.to_string(),
        errors: Vec::new(),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
