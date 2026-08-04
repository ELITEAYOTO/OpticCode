use anyhow::{bail, Context, Result};
use opticcode_policy::{
    ApplyPatchAction, NativeConfirmation, PolicyAction, PolicyDecision, PolicyEngine, PolicyMode,
    DEFAULT_APPROVAL_TTL_SECONDS,
};
use opticcode_tools::apply_transaction::{
    execute_apply_transaction, rollback_apply_transaction, ApplyGitPolicy, ApplyTransactionRequest,
};
use opticcode_tools::process_runner::CancellationToken;

use crate::policy_adapter::{observe_repository, policy_record, PolicySession};
use crate::transaction::{
    proposal_files_hash, proposal_mutations, proposal_paths, reparse_java_files,
    verify_snapshots_at,
};
use crate::{
    capture_verified_diff, content_hash, new_edit_id, unix_millis, ChatEditApplyReport,
    EditRuntimeOptions, EditStageReport, EditStageStatus, PolicyDecisionRecord, ProposalRecord,
    ProposalState, ProposalStore, CHAT_EDIT_REPORT_SCHEMA_VERSION,
};

pub fn apply_verified_proposal(
    store: &ProposalStore,
    proposal_id: &str,
    policy: &PolicyEngine,
    options: &EditRuntimeOptions,
    confirmation: &NativeConfirmation,
    cancellation: &CancellationToken,
) -> Result<ProposalRecord> {
    let transaction_id = new_edit_id("apply")?;
    apply_verified_proposal_with_transaction_id(
        store,
        proposal_id,
        policy,
        options,
        confirmation,
        cancellation,
        &transaction_id,
    )
}

pub fn apply_verified_proposal_with_transaction_id(
    store: &ProposalStore,
    proposal_id: &str,
    policy: &PolicyEngine,
    options: &EditRuntimeOptions,
    confirmation: &NativeConfirmation,
    cancellation: &CancellationToken,
    transaction_id: &str,
) -> Result<ProposalRecord> {
    let record = store.expire_if_needed(proposal_id, unix_millis())?;
    validate_apply_preconditions(&record, options)?;
    let observed = observe_repository(options, cancellation)?;
    if !observed.clean
        || observed.boundary.head != record.plan.base_head
        || observed.working_tree_digest != record.plan.working_tree_digest
        || observed.root_hash != record.workspace_root_hash
    {
        bail!("workspace drifted after verification; re-verify before applying");
    }
    verify_snapshots_at(&observed.root, &record.files, false)?;
    let diff = record
        .verified_diff
        .clone()
        .context("verified proposal has no stored diff")?;
    if content_hash(diff.patch.as_bytes()) != diff.patch_hash {
        bail!("stored verified diff failed its integrity check");
    }

    let transaction_id = transaction_id.to_string();
    let (paths, created_paths) = proposal_paths(&record.files);
    let action = PolicyAction::ApplyPatch(ApplyPatchAction {
        root: observed.root.clone(),
        paths,
        created_paths,
        diff_hash: diff.patch_hash.clone(),
        files_hash: proposal_files_hash(&record.files),
        transaction_id: transaction_id.clone(),
        base_head: record.plan.base_head.clone(),
    });
    let session = PolicySession::new(policy, options, &observed);
    let mut decisions = Vec::<PolicyDecisionRecord>::new();
    let approval_request = session.request(
        PolicyMode::ApprovedApply,
        action.clone(),
        None,
        None,
        "apply_approval",
        decisions.len(),
    );
    let approval_preflight = policy
        .check(&approval_request)
        .context("failed to evaluate original apply approval")?;
    decisions.push(policy_record("apply_approval", &approval_preflight.report));
    if !matches!(
        approval_preflight.report.decision,
        PolicyDecision::RequireApproval { .. }
    ) {
        bail!(
            "original apply did not produce the required approval decision: {}",
            approval_preflight.report.decision.rule_id()
        );
    }
    store.transition(
        proposal_id,
        ProposalState::ApprovalPending,
        "native confirmation received; issuing state-bound one-shot approval",
    )?;
    let grant = match policy.issue_approval(
        &approval_request,
        confirmation,
        DEFAULT_APPROVAL_TTL_SECONDS,
    ) {
        Ok(grant) => grant,
        Err(error) => {
            store.transition(
                proposal_id,
                ProposalState::Verified,
                "approval issuance failed before any workspace mutation",
            )?;
            return Err(error).context("failed to issue one-shot apply approval");
        }
    };
    store.transition(
        proposal_id,
        ProposalState::Applying,
        format!("applying with transaction {}", transaction_id),
    )?;
    if let Err(error) = session.authorize(
        PolicyMode::ApprovedApply,
        action,
        None,
        Some(grant.approval_id.clone()),
        "apply_consume",
        &mut decisions,
    ) {
        store.transition(
            proposal_id,
            ProposalState::Verified,
            "one-shot approval was not consumed; original workspace remained unchanged",
        )?;
        store.record_policy(proposal_id, decisions)?;
        return Err(error);
    }

    let transaction = execute_apply_transaction(
        ApplyTransactionRequest::new(
            &observed.root,
            diff.patch.as_bytes().to_vec(),
            proposal_mutations(&record.files),
        )
        .with_git_policy(ApplyGitPolicy::RequireClean)
        .with_transaction_id(&transaction_id),
    );
    let mut report = ChatEditApplyReport {
        schema_version: CHAT_EDIT_REPORT_SCHEMA_VERSION,
        request_id: options.request_id.clone(),
        proposal_id: proposal_id.to_string(),
        transaction_id: transaction_id.clone(),
        success: false,
        approval_id: grant.approval_id,
        approval_consumed: true,
        post_reparse: not_run("post-apply Tree-sitter reparse did not run"),
        post_build: not_run("post-apply validation did not run"),
        rollback_attempted: false,
        rollback_success: None,
        policy: Vec::new(),
        applied_at_unix_ms: None,
        errors: Vec::new(),
    };
    let transaction = match transaction {
        Ok(transaction) if transaction.committed() => transaction,
        Ok(transaction) => {
            report.rollback_attempted = transaction.rollback_attempted;
            report.rollback_success = transaction.rollback_success;
            report.errors.extend(transaction.errors);
            let workspace_restored =
                verify_snapshots_at(&observed.root, &record.files, false).is_ok();
            finish_failed_apply(store, proposal_id, decisions, report, workspace_restored)?;
            return store.load(proposal_id);
        }
        Err(error) => {
            report.errors.push(format!("APPLY-001 failed: {error:#}"));
            let workspace_restored =
                verify_snapshots_at(&observed.root, &record.files, false).is_ok();
            finish_failed_apply(store, proposal_id, decisions, report, workspace_restored)?;
            return store.load(proposal_id);
        }
    };

    report.post_reparse = reparse_java_files(&observed.root, &record.files);
    let post_diff = capture_verified_diff(
        &observed.root,
        &record.files,
        options.git_timeout,
        cancellation,
    );
    let exact_diff = match post_diff {
        Ok(actual) if actual == diff => true,
        Ok(_) => {
            report
                .errors
                .push("post-apply Git diff differs from the verified diff".to_string());
            false
        }
        Err(error) => {
            report
                .errors
                .push(format!("post-apply Git verification failed: {error:#}"));
            false
        }
    };
    let prior_build_passed = record
        .verification
        .as_ref()
        .is_some_and(|verification| verification.build.status == EditStageStatus::Passed);
    report.post_build = if prior_build_passed && exact_diff {
        EditStageReport::passed(
            "exact verified snapshots retain the successful offline worktree build result",
            0,
        )
    } else {
        EditStageReport::failed(
            "post-apply build binding failed",
            0,
            "the verified offline build or exact post-apply diff is unavailable",
        )
    };

    let postconditions_passed = report.post_reparse.status == EditStageStatus::Passed
        && report.post_build.status == EditStageStatus::Passed
        && exact_diff;
    if !postconditions_passed {
        report.rollback_attempted = true;
        match rollback_apply_transaction(&observed.root, &transaction.transaction_id) {
            Ok(rollback) => {
                let rolled_back = rollback.rolled_back();
                report.rollback_success = Some(rolled_back);
                report.errors.extend(rollback.errors);
                if rolled_back && verify_snapshots_at(&observed.root, &record.files, false).is_ok()
                {
                    store.transition(
                        proposal_id,
                        ProposalState::Verified,
                        "post-apply validation failed and automatic rollback restored the base",
                    )?;
                } else {
                    store.transition(
                        proposal_id,
                        ProposalState::Failed,
                        "post-apply validation and automatic rollback failed",
                    )?;
                }
            }
            Err(error) => {
                report.rollback_success = Some(false);
                report
                    .errors
                    .push(format!("automatic rollback failed: {error:#}"));
                store.transition(
                    proposal_id,
                    ProposalState::Failed,
                    "post-apply validation failed and rollback could not complete",
                )?;
            }
        }
    } else {
        report.success = true;
        report.applied_at_unix_ms = Some(unix_millis());
        store.transition(
            proposal_id,
            ProposalState::Applied,
            format!(
                "transaction {} committed and passed postconditions",
                transaction_id
            ),
        )?;
        store.transition(
            proposal_id,
            ProposalState::RollbackAvailable,
            format!(
                "transaction {} is available for exact rollback",
                transaction_id
            ),
        )?;
    }
    report.policy = decisions.clone();
    store.record_policy(proposal_id, decisions)?;
    store.record_apply(proposal_id, report)
}

fn validate_apply_preconditions(
    record: &ProposalRecord,
    options: &EditRuntimeOptions,
) -> Result<()> {
    if record.state != ProposalState::Verified
        || record.workspace_id != options.workspace_id
        || record.expired(unix_millis())
        || !record
            .verification
            .as_ref()
            .is_some_and(|report| report.success && !report.lease_recovery_required)
    {
        bail!("proposal is not a current, successful, cleanup-complete verification");
    }
    Ok(())
}

fn finish_failed_apply(
    store: &ProposalStore,
    proposal_id: &str,
    decisions: Vec<PolicyDecisionRecord>,
    mut report: ChatEditApplyReport,
    workspace_unchanged_or_restored: bool,
) -> Result<()> {
    store.transition(
        proposal_id,
        if workspace_unchanged_or_restored {
            ProposalState::Verified
        } else {
            ProposalState::Failed
        },
        "transactional apply failed closed",
    )?;
    report.policy = decisions.clone();
    store.record_policy(proposal_id, decisions)?;
    store.record_apply(proposal_id, report)?;
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
