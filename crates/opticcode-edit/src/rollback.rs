use anyhow::{bail, Context, Result};
use opticcode_policy::{
    NativeConfirmation, PolicyAction, PolicyDecision, PolicyEngine, PolicyMode, TransactionAction,
    DEFAULT_APPROVAL_TTL_SECONDS,
};
use opticcode_tools::apply_transaction::rollback_apply_transaction;
use opticcode_tools::process_runner::CancellationToken;

use crate::policy_adapter::{observe_repository, policy_record, PolicySession};
use crate::transaction::{reparse_java_files, verify_snapshots_at};
use crate::{
    capture_verified_diff, unix_millis, ChatEditRollbackReport, EditRuntimeOptions,
    EditStageReport, EditStageStatus, PolicyDecisionRecord, ProposalRecord, ProposalState,
    ProposalStore, CHAT_EDIT_REPORT_SCHEMA_VERSION,
};

pub fn rollback_edit_proposal(
    store: &ProposalStore,
    proposal_id: &str,
    policy: &PolicyEngine,
    options: &EditRuntimeOptions,
    confirmation: &NativeConfirmation,
    cancellation: &CancellationToken,
) -> Result<ProposalRecord> {
    let record = store.load(proposal_id)?;
    if record.state == ProposalState::RolledBack {
        return Ok(record);
    }
    if !matches!(
        record.state,
        ProposalState::RollbackAvailable | ProposalState::Applied
    ) || record.workspace_id != options.workspace_id
    {
        bail!("proposal does not expose a rollbackable transaction in this workspace");
    }
    let apply = record
        .apply
        .as_ref()
        .filter(|report| report.success)
        .context("proposal has no successful apply report")?;
    let diff = record
        .verified_diff
        .as_ref()
        .context("proposal has no verified diff for rollback binding")?;
    let observed = observe_repository(options, cancellation)?;
    if observed.boundary.head != record.plan.base_head
        || observed.root_hash != record.workspace_root_hash
    {
        bail!("workspace identity or HEAD changed after the applied transaction");
    }
    verify_snapshots_at(&observed.root, &record.files, true)?;
    let current_diff = capture_verified_diff(
        &observed.root,
        &record.files,
        options.git_timeout,
        cancellation,
    )?;
    if &current_diff != diff {
        bail!("workspace no longer matches the exact applied transaction diff");
    }

    let action = PolicyAction::RollbackTransaction(TransactionAction {
        workspace_root: observed.root.clone(),
        transaction_id: apply.transaction_id.clone(),
        expected_state_hash: diff.patch_hash.clone(),
    });
    let session = PolicySession::new(policy, options, &observed);
    let mut decisions = Vec::<PolicyDecisionRecord>::new();
    let approval_request = session.request(
        PolicyMode::ApprovedApply,
        action.clone(),
        None,
        None,
        "rollback_approval",
        decisions.len(),
    );
    let preflight = policy
        .check(&approval_request)
        .context("failed to evaluate rollback approval")?;
    decisions.push(policy_record("rollback_approval", &preflight.report));
    if !matches!(
        preflight.report.decision,
        PolicyDecision::RequireApproval { .. }
    ) {
        bail!(
            "rollback did not produce the required approval decision: {}",
            preflight.report.decision.rule_id()
        );
    }
    let grant = policy.issue_approval(
        &approval_request,
        confirmation,
        DEFAULT_APPROVAL_TTL_SECONDS,
    )?;
    store.transition(
        proposal_id,
        ProposalState::RollingBack,
        format!("rolling back exact transaction {}", apply.transaction_id),
    )?;
    if let Err(error) = session.authorize(
        PolicyMode::ApprovedApply,
        action,
        None,
        Some(grant.approval_id.clone()),
        "rollback_consume",
        &mut decisions,
    ) {
        store.transition(
            proposal_id,
            ProposalState::RollbackAvailable,
            "rollback approval was not consumed; workspace remained applied",
        )?;
        store.record_policy(proposal_id, decisions)?;
        return Err(error);
    }

    let rollback = rollback_apply_transaction(&observed.root, &apply.transaction_id);
    let mut report = ChatEditRollbackReport {
        schema_version: CHAT_EDIT_REPORT_SCHEMA_VERSION,
        request_id: options.request_id.clone(),
        proposal_id: proposal_id.to_string(),
        transaction_id: apply.transaction_id.clone(),
        success: false,
        already_rolled_back: false,
        approval_id: grant.approval_id,
        approval_consumed: true,
        reparse: not_run("post-rollback Tree-sitter reparse did not run"),
        policy: Vec::new(),
        rolled_back_at_unix_ms: None,
        errors: Vec::new(),
    };
    match rollback {
        Ok(result) if result.rolled_back() => {
            verify_snapshots_at(&observed.root, &record.files, false)?;
            report.reparse = reparse_java_files(&observed.root, &record.files);
            let after = observe_repository(options, cancellation)?;
            if report.reparse.status == EditStageStatus::Passed
                && after.clean
                && after.boundary.head == record.plan.base_head
                && after.working_tree_digest == record.plan.working_tree_digest
            {
                report.success = true;
                report.rolled_back_at_unix_ms = Some(unix_millis());
                store.transition(
                    proposal_id,
                    ProposalState::RolledBack,
                    format!("transaction {} rolled back exactly", apply.transaction_id),
                )?;
            } else {
                report.errors.push(
                    "rollback completed but source postconditions did not match the base proposal"
                        .to_string(),
                );
                store.transition(
                    proposal_id,
                    ProposalState::Failed,
                    "transaction rolled back but postconditions failed",
                )?;
            }
        }
        Ok(result) => {
            report.errors.extend(result.errors);
            store.transition(
                proposal_id,
                ProposalState::RollbackAvailable,
                "transaction rollback failed and remains recoverable",
            )?;
        }
        Err(error) => {
            report
                .errors
                .push(format!("APPLY-001 rollback failed: {error:#}"));
            store.transition(
                proposal_id,
                ProposalState::RollbackAvailable,
                "transaction rollback could not start and remains recoverable",
            )?;
        }
    }
    report.policy = decisions.clone();
    store.record_policy(proposal_id, decisions)?;
    store.record_rollback(proposal_id, report)
}

fn not_run(summary: &str) -> EditStageReport {
    EditStageReport {
        status: EditStageStatus::NotRun,
        duration_ms: 0,
        summary: summary.to_string(),
        errors: Vec::new(),
    }
}
