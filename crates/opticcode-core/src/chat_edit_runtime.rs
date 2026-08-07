use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use opticcode_edit::{
    apply_verified_proposal_with_transaction_id, build_bound_edit_generation_prompt,
    build_bound_format_correction_prompt, canonical_root_hash, edit_plan_output_schema_for_intent,
    inspect_edit_workspace, new_edit_id, parse_edit_plan_json, rollback_edit_proposal,
    validate_edit_intent, validate_edit_plan, validate_edit_plan_against_intent,
    verify_edit_proposal, BoundEditGenerationInput, ChatEditVerificationReport,
    EditGenerationInput, EditIntent, EditIntentAllowedExistingTarget, EditIntentConstraints,
    EditIntentExpectations, EditIntentSelectionMode, EditIntentTarget, EditIntentTargetProvenance,
    EditPlanError, EditPlanExpectations, EditPlanLimits, EditRuntimeOptions, EditStageStatus,
    PolicyDecisionRecord, ProposalFileStatus, ProposalRecord, ProposalState, ProposalStore,
    TrustedEditFile, TrustedEditLine, ValidatedEditIntent, ValidatedEditPlan,
    ALLOWED_EDIT_EXTENSIONS, DEFAULT_EDIT_INTENT_TTL_SECONDS, EDIT_INTENT_SCHEMA_VERSION,
    MAX_EDIT_FILE_BYTES,
};
use opticcode_llm::GenerationResult;
use opticcode_policy::{NativeConfirmation, PolicyEngine};
use opticcode_tools::inspect_workspace;
use opticcode_tools::process_runner::CancellationToken as ToolCancellationToken;
use opticcode_tools::rag::read_safe_workspace_file;

use crate::chat_protocol::{
    ChatCommand, ChatEditReviewFile, ChatEventEmitter, ChatMetrics, ChatProtocolEventPayload,
    ChatReferenceTarget, ChatRequest,
};
use crate::chat_runtime::{
    emit_text, ChatPolicyAuthorization, ChatRuntimeOptions, CommandOutcome, PreparedRequest,
    RuntimeFailure,
};
use crate::{ContextMode, GroundingRoute, OpticCode};

const EDIT_POLICY_SUMMARY: &str = concat!(
    "Deny by default. Existing allowlisted UTF-8 text modifications and at most one ",
    "allowlisted UTF-8 text creation may be verified only in an owned disposable worktree. ",
    "No delete, rename, binary, secret, wrapper, network, dependency installation, shell, ",
    "symlink, junction, reparse point, or path outside the workspace is permitted. ",
    "Original workspace apply and rollback require native confirmation and one-shot Policy approval."
);

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_chat_edit(
    app: Option<&OpticCode>,
    request: &ChatRequest,
    prepared: &PreparedRequest,
    initial_policy: &ChatPolicyAuthorization,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
    _started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    match request.command {
        ChatCommand::Fix => {
            let app = app.ok_or_else(|| {
                RuntimeFailure::new(
                    "provider_unavailable",
                    "edit_generation",
                    "the configured local LLM provider is unavailable",
                )
                .retriable(true)
            })?;
            run_fix(
                app,
                request,
                prepared,
                initial_policy,
                emitter,
                cancellation,
                options,
            )
            .await
        }
        ChatCommand::Verify => run_verify(request, prepared, emitter, cancellation, options).await,
        ChatCommand::Diff => run_diff(request, prepared, emitter, options).await,
        ChatCommand::Apply => run_apply(request, prepared, emitter, cancellation, options).await,
        ChatCommand::Rollback => {
            run_rollback(request, prepared, emitter, cancellation, options).await
        }
        _ => Err(RuntimeFailure::new(
            "invalid_edit_command",
            "edit_dispatch",
            "chat edit runtime received a non-edit command",
        )),
    }
}

async fn run_fix(
    app: &OpticCode,
    request: &ChatRequest,
    prepared: &PreparedRequest,
    initial_policy: &ChatPolicyAuthorization,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let runtime = edit_options(request, prepared);
    let observed = run_blocking_edit(cancellation, "edit_observation", {
        let runtime = runtime.clone();
        move |tool_cancel| inspect_edit_workspace(&runtime, tool_cancel)
    })
    .await?;
    if !observed.clean {
        return Err(RuntimeFailure::new(
            "workspace_not_clean",
            "edit_observation",
            "edit proposals require a clean main Git worktree",
        ));
    }
    let store = proposal_store(options, &observed.root_hash).map_err(|error| {
        edit_failure(
            "proposal_store",
            "proposal store could not be opened",
            error,
        )
    })?;
    let plan_id = new_edit_id("plan")
        .map_err(|error| edit_failure("edit_generation", "plan ID creation failed", error))?;
    let expected = EditPlanExpectations {
        request_id: request.request_id.clone(),
        plan_id: plan_id.clone(),
        workspace_id: request.workspace_id.clone(),
        workspace_root: observed.root.clone(),
        workspace_root_hash: observed.root_hash.clone(),
        profile: request.profile.clone(),
        provider: request.provider,
        model: request.model.clone(),
        base_head: observed.base_head.clone(),
        working_tree_digest: observed.working_tree_digest.clone(),
        now_unix_ms: opticcode_edit::unix_millis(),
        limits: EditPlanLimits::default(),
    };
    let available_files = available_edit_files(&observed.root).map_err(|error| {
        edit_failure(
            "edit_context",
            "available edit file inventory failed",
            error,
        )
    })?;
    let trusted_files =
        trusted_edit_files(&observed.root, &prepared.references).map_err(|error| {
            edit_failure("edit_context", "trusted edit file inventory failed", error)
        })?;
    let intent_id = new_edit_id("intent")
        .map_err(|error| edit_failure("edit_intent", "intent ID creation failed", error))?;
    emitter
        .send(ChatProtocolEventPayload::EditIntentStarted {
            intent_id: intent_id.clone(),
            intent_schema_version: EDIT_INTENT_SCHEMA_VERSION,
        })
        .await
        .map_err(event_failure)?;
    let intent = build_runtime_edit_intent(
        request,
        &expected,
        &trusted_files,
        &prepared.references,
        intent_id,
    )
    .map_err(|error| edit_failure("edit_intent", "trusted edit intent creation failed", error))?;
    emitter
        .send(ChatProtocolEventPayload::EditIntentReady {
            intent_id: intent.intent.intent_id.clone(),
            intent_schema_version: intent.intent.schema_version,
            intent_hash: intent.intent_hash.clone(),
            selection_mode: edit_intent_selection_mode_name(intent.intent.selection_mode)
                .to_string(),
            target_count: intent.intent.targets.len(),
            expires_at_unix_ms: intent.intent.expires_at_unix_ms,
        })
        .await
        .map_err(event_failure)?;
    let user_references = serde_json::to_string(&request.references).map_err(|error| {
        edit_failure(
            "edit_context",
            "user reference serialization failed",
            error.into(),
        )
    })?;
    let generation_prompt = build_bound_edit_generation_prompt(BoundEditGenerationInput {
        base: EditGenerationInput {
            task: &request.prompt,
            policy_summary: EDIT_POLICY_SUMMARY,
            selected_context: &prepared.prompt,
            user_references: &user_references,
            available_files: &available_files,
            trusted_files: &trusted_files,
            expected: &expected,
        },
        intent: &intent,
    })
    .map_err(|error| edit_failure("edit_generation", "edit prompt creation failed", error))?;

    emitter
        .send(ChatProtocolEventPayload::EditPlanStarted {
            plan_id: plan_id.clone(),
        })
        .await
        .map_err(event_failure)?;
    emitter
        .send(ChatProtocolEventPayload::ProviderStarted {
            provider: request.provider,
            model: request.model.clone(),
            context_mode: request.context_mode,
        })
        .await
        .map_err(event_failure)?;
    let (validated, generation, corrected) = generate_validated_plan(
        app,
        request,
        &expected,
        &trusted_files,
        &intent,
        generation_prompt,
        cancellation,
    )
    .await?;
    if corrected {
        emitter
            .send(ChatProtocolEventPayload::Warning {
                code: "edit_plan_format_corrected".to_string(),
                message: "The single allowed schema-format correction produced a valid EditPlan."
                    .to_string(),
            })
            .await
            .map_err(event_failure)?;
    }

    let summary = validated.plan.summary.clone();
    let file_count = validated.files.len();
    let record = store
        .create_with_intent(validated, &intent)
        .map_err(|error| {
            edit_failure(
                "proposal_store",
                "validated proposal could not be published",
                error,
            )
        })?;
    let generation_policy = PolicyDecisionRecord {
        stage: "generation_context".to_string(),
        action_kind: initial_policy.action_kind.clone(),
        decision: initial_policy.decision.clone(),
        rule_id: initial_policy.rule_id.clone(),
        action_hash: initial_policy.action_hash.clone(),
        audit_event_id: initial_policy.audit_event_id.clone(),
    };
    store
        .record_policy(&record.proposal_id, vec![generation_policy.clone()])
        .map_err(|error| {
            edit_failure(
                "proposal_store",
                "generation Policy reference could not be stored",
                error,
            )
        })?;
    emit_policy(emitter, Some(&record.proposal_id), &generation_policy).await?;
    emitter
        .send(ChatProtocolEventPayload::EditPlanReady {
            plan_id,
            summary,
            file_count,
        })
        .await
        .map_err(event_failure)?;
    let stored_intent = record.intent.as_ref().ok_or_else(|| {
        RuntimeFailure::new(
            "proposal_intent_missing",
            "proposal_store",
            "intent-bound proposal was stored without its trusted intent binding",
        )
    })?;
    emitter
        .send(ChatProtocolEventPayload::ProposalStored {
            proposal_id: record.proposal_id.clone(),
            state: state_name(record.state),
            expires_at_unix_ms: record.expires_at_unix_ms,
            intent_id: stored_intent.intent_id.clone(),
            intent_schema_version: stored_intent.schema_version,
            intent_hash: stored_intent.intent_hash.clone(),
        })
        .await
        .map_err(event_failure)?;

    let verified = verify_and_emit(
        &store,
        &record.proposal_id,
        request,
        prepared,
        emitter,
        cancellation,
        options,
    )
    .await?;
    Ok(outcome_from_generation(
        request.context_mode,
        &generation,
        &verified,
    ))
}

async fn run_verify(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let (store, record) = resolve_proposal(request, prepared, options, false)?;
    let verified = verify_and_emit(
        &store,
        &record.proposal_id,
        request,
        prepared,
        emitter,
        cancellation,
        options,
    )
    .await?;
    Ok(edit_outcome(
        None,
        vec![verification_warning(&verified)]
            .into_iter()
            .flatten()
            .collect(),
    ))
}

async fn run_diff(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    options: &ChatRuntimeOptions,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let (store, record) = resolve_proposal(request, prepared, options, false)?;
    if request.edit.as_ref().is_some_and(|edit| edit.discard) {
        let discarded = store.discard(&record.proposal_id).map_err(|error| {
            edit_failure("proposal_discard", "proposal could not be discarded", error)
        })?;
        emitter
            .send(ChatProtocolEventPayload::ProposalDiscarded {
                proposal_id: discarded.proposal_id.clone(),
            })
            .await
            .map_err(event_failure)?;
        emit_text(
            emitter,
            &format!(
                "Proposal `{}` was discarded locally.",
                discarded.proposal_id
            ),
        )
        .await?;
        return Ok(edit_outcome(None, Vec::new()));
    }
    let record = store
        .expire_if_needed(&record.proposal_id, opticcode_edit::unix_millis())
        .map_err(|error| edit_failure("proposal_store", "proposal expiry check failed", error))?;
    ensure_reviewable(&record)?;
    emit_diff(emitter, &record).await?;
    if record.state == ProposalState::Verified {
        emit_approval_required(emitter, &record, "apply").await?;
    }
    emit_text(emitter, &diff_markdown(&record)).await?;
    Ok(edit_outcome(None, Vec::new()))
}

async fn run_apply(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let (store, record) = resolve_proposal(request, prepared, options, false)?;
    ensure_reviewable(&record)?;
    let expected_confirmation = approval_request_id(&record, "apply").map_err(|error| {
        edit_failure(
            "approval_precondition",
            "apply approval binding could not be created",
            error,
        )
    })?;
    let Some(confirmation) = request
        .edit
        .as_ref()
        .and_then(|edit| edit.native_confirmation.as_ref())
    else {
        emit_diff(emitter, &record).await?;
        emit_approval_required(emitter, &record, "apply").await?;
        emit_text(
            emitter,
            "The verified proposal is ready. Use the native **Apply Verified Changes** confirmation; typed Chat consent never applies files.",
        )
        .await?;
        return Ok(edit_outcome(None, Vec::new()));
    };
    validate_native_confirmation(request, confirmation, &expected_confirmation)?;
    let native = NativeConfirmation::explicit(
        confirmation.client.clone(),
        confirmation.confirmation_id.clone(),
    )
    .map_err(|error| {
        edit_failure(
            "native_confirmation",
            "native apply confirmation is invalid",
            error,
        )
    })?;
    let transaction_id = new_edit_id("apply").map_err(|error| {
        edit_failure(
            "apply_transaction",
            "transaction identity creation failed",
            error,
        )
    })?;
    emitter
        .send(ChatProtocolEventPayload::ApplyStarted {
            proposal_id: record.proposal_id.clone(),
            transaction_id: transaction_id.clone(),
        })
        .await
        .map_err(event_failure)?;
    let policy_offset = record.policy.len();
    let applied = run_blocking_edit(cancellation, "apply_transaction", {
        let store = store.clone();
        let proposal_id = record.proposal_id.clone();
        let runtime = edit_options(request, prepared);
        let policy_root = options.policy_state_root.clone();
        let transaction_id = transaction_id.clone();
        move |tool_cancel| {
            let policy = open_policy(policy_root)?;
            apply_verified_proposal_with_transaction_id(
                &store,
                &proposal_id,
                &policy,
                &runtime,
                &native,
                tool_cancel,
                &transaction_id,
            )
        }
    })
    .await?;
    emit_policy_records(emitter, &applied, policy_offset).await?;
    let report = applied
        .apply
        .as_ref()
        .context("apply completed without a persisted report")
        .map_err(|error| edit_failure("apply_report", "apply report is missing", error))?;
    emitter
        .send(ChatProtocolEventPayload::ApplyCompleted {
            proposal_id: applied.proposal_id.clone(),
            transaction_id: report.transaction_id.clone(),
            success: report.success,
        })
        .await
        .map_err(event_failure)?;
    if report.success {
        emitter
            .send(ChatProtocolEventPayload::RollbackAvailable {
                proposal_id: applied.proposal_id.clone(),
                transaction_id: report.transaction_id.clone(),
            })
            .await
            .map_err(event_failure)?;
    }
    emit_text(
        emitter,
        if report.success {
            "The verified transaction was applied to the original workspace. Exact rollback is available."
        } else {
            "The original apply failed closed; inspect the persisted report before retrying."
        },
    )
    .await?;
    Ok(edit_outcome(
        None,
        (!report.success)
            .then(|| "transactional apply did not complete".to_string())
            .into_iter()
            .collect(),
    ))
}

async fn run_rollback(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let (store, record) = resolve_proposal(request, prepared, options, true)?;
    let transaction_id = record
        .apply
        .as_ref()
        .map(|report| report.transaction_id.clone())
        .context("proposal has no applied transaction")
        .map_err(|error| {
            edit_failure(
                "rollback_precondition",
                "rollback transaction is unavailable",
                error,
            )
        })?;
    let expected_confirmation = approval_request_id(&record, "rollback").map_err(|error| {
        edit_failure(
            "approval_precondition",
            "rollback approval binding could not be created",
            error,
        )
    })?;
    let Some(confirmation) = request
        .edit
        .as_ref()
        .and_then(|edit| edit.native_confirmation.as_ref())
    else {
        emit_approval_required(emitter, &record, "rollback").await?;
        emit_text(
            emitter,
            "Exact rollback is ready. Use the native **Rollback Transaction** confirmation; typed Chat consent is ignored.",
        )
        .await?;
        return Ok(edit_outcome(None, Vec::new()));
    };
    validate_native_confirmation(request, confirmation, &expected_confirmation)?;
    let native = NativeConfirmation::explicit(
        confirmation.client.clone(),
        confirmation.confirmation_id.clone(),
    )
    .map_err(|error| {
        edit_failure(
            "native_confirmation",
            "native rollback confirmation is invalid",
            error,
        )
    })?;
    let already_rolled_back = record.state == ProposalState::RolledBack;
    emitter
        .send(ChatProtocolEventPayload::RollbackStarted {
            proposal_id: record.proposal_id.clone(),
            transaction_id: transaction_id.clone(),
        })
        .await
        .map_err(event_failure)?;
    let policy_offset = record.policy.len();
    let rolled_back = run_blocking_edit(cancellation, "rollback_transaction", {
        let store = store.clone();
        let proposal_id = record.proposal_id.clone();
        let runtime = edit_options(request, prepared);
        let policy_root = options.policy_state_root.clone();
        move |tool_cancel| {
            let policy = open_policy(policy_root)?;
            rollback_edit_proposal(
                &store,
                &proposal_id,
                &policy,
                &runtime,
                &native,
                tool_cancel,
            )
        }
    })
    .await?;
    emit_policy_records(emitter, &rolled_back, policy_offset).await?;
    let success = rolled_back.state == ProposalState::RolledBack;
    emitter
        .send(ChatProtocolEventPayload::RollbackCompleted {
            proposal_id: rolled_back.proposal_id.clone(),
            transaction_id,
            success,
            already_rolled_back,
        })
        .await
        .map_err(event_failure)?;
    emit_text(
        emitter,
        if success {
            "The exact OpticCode transaction was rolled back and the base snapshots were restored."
        } else {
            "Rollback did not complete; the targeted transaction remains recoverable."
        },
    )
    .await?;
    Ok(edit_outcome(
        None,
        (!success)
            .then(|| "transaction rollback did not complete".to_string())
            .into_iter()
            .collect(),
    ))
}

async fn generate_validated_plan(
    app: &OpticCode,
    request: &ChatRequest,
    expected: &EditPlanExpectations,
    trusted_files: &[TrustedEditFile],
    intent: &ValidatedEditIntent,
    prompt: String,
    cancellation: &opticcode_llm::CancellationToken,
) -> std::result::Result<(ValidatedEditPlan, GenerationResult, bool), RuntimeFailure> {
    let generation_id = new_edit_id("llm").map_err(|error| {
        edit_failure(
            "edit_generation",
            "generation identity creation failed",
            error,
        )
    })?;
    let output_schema = edit_plan_output_schema_for_intent(expected, trusted_files, intent)
        .map_err(|error| {
            edit_failure(
                "edit_intent_schema",
                "intent-bound output schema creation failed",
                error,
            )
        })?;
    let primary = app
        .generate_structured(
            generation_id,
            prompt,
            output_schema.clone(),
            request.generation.max_output_tokens,
            Some(request.generation.temperature.unwrap_or(0.0)),
            request.generation.seed,
            cancellation.clone(),
        )
        .await
        .map_err(|error| {
            edit_failure(
                "edit_generation",
                "structured local generation failed",
                error,
            )
            .retriable(true)
        })?;
    match parse_and_validate(&primary.output, expected) {
        Ok(validated) => {
            validate_plan_intent_binding(&validated, intent)?;
            Ok((validated, primary, false))
        }
        Err(error) if format_correctable(&error) => {
            let correction_prompt = build_bound_format_correction_prompt(
                &primary.output,
                &error.code,
                &error.message,
                expected,
                trusted_files,
                intent,
            )
            .map_err(|failure| {
                edit_failure(
                    "edit_format_correction",
                    "format correction prompt failed",
                    failure,
                )
            })?;
            let correction_id = new_edit_id("llm-correction").map_err(|failure| {
                edit_failure(
                    "edit_format_correction",
                    "correction identity creation failed",
                    failure,
                )
            })?;
            let corrected = app
                .generate_structured(
                    correction_id,
                    correction_prompt,
                    output_schema,
                    request.generation.max_output_tokens,
                    Some(0.0),
                    request.generation.seed,
                    cancellation.clone(),
                )
                .await
                .map_err(|failure| {
                    edit_failure(
                        "edit_format_correction",
                        "single format correction generation failed",
                        failure,
                    )
                })?;
            let validated = parse_and_validate(&corrected.output, expected).map_err(|failure| {
                RuntimeFailure::new(
                    "edit_plan_invalid",
                    "edit_validation",
                    format!(
                        "the single allowed correction remained invalid: {}: {}",
                        failure.code, failure.message
                    ),
                )
            })?;
            validate_plan_intent_binding(&validated, intent)?;
            Ok((validated, corrected, true))
        }
        Err(error) => Err(RuntimeFailure::new(
            "edit_plan_invalid",
            "edit_validation",
            format!("{}: {}", error.code, error.message),
        )),
    }
}

fn parse_and_validate(
    output: &str,
    expected: &EditPlanExpectations,
) -> std::result::Result<ValidatedEditPlan, EditPlanError> {
    let plan = parse_edit_plan_json(output)?;
    validate_edit_plan(plan, expected)
}

fn validate_plan_intent_binding(
    plan: &ValidatedEditPlan,
    intent: &ValidatedEditIntent,
) -> std::result::Result<(), RuntimeFailure> {
    validate_edit_plan_against_intent(plan, intent).map_err(|error| {
        RuntimeFailure::new(
            "edit_plan_intent_mismatch",
            "edit_intent_validation",
            format!("{}: {}", error.code, error.message),
        )
    })
}
fn format_correctable(error: &EditPlanError) -> bool {
    matches!(
        error.code.as_str(),
        "plan.trailing_text" | "plan.invalid_json"
    )
}

async fn verify_and_emit(
    store: &ProposalStore,
    proposal_id: &str,
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
) -> std::result::Result<ProposalRecord, RuntimeFailure> {
    let before = store.load(proposal_id).map_err(|error| {
        edit_failure(
            "proposal_store",
            "proposal could not be loaded before verification",
            error,
        )
    })?;
    emitter
        .send(ChatProtocolEventPayload::VerificationStarted {
            proposal_id: proposal_id.to_string(),
        })
        .await
        .map_err(event_failure)?;
    let verified = run_blocking_edit(cancellation, "edit_verification", {
        let store = store.clone();
        let proposal_id = proposal_id.to_string();
        let runtime = edit_options(request, prepared);
        let policy_root = options.policy_state_root.clone();
        move |tool_cancel| {
            let policy = open_policy(policy_root)?;
            verify_edit_proposal(&store, &proposal_id, &policy, &runtime, tool_cancel)
        }
    })
    .await?;
    emit_policy_records(emitter, &verified, before.policy.len()).await?;
    let report = verified
        .verification
        .as_ref()
        .context("verification completed without a persisted report")
        .map_err(|error| {
            edit_failure(
                "verification_report",
                "verification report is missing",
                error,
            )
        })?;
    emit_verification_stages(emitter, &verified, report).await?;
    if verified.verified_diff.is_some() {
        emit_diff(emitter, &verified).await?;
    }
    emitter
        .send(ChatProtocolEventPayload::VerificationCompleted {
            proposal_id: verified.proposal_id.clone(),
            success: report.success,
            build: stage_name(report.build.status),
            tests: stage_name(report.tests.status),
        })
        .await
        .map_err(event_failure)?;
    if report.success {
        emit_approval_required(emitter, &verified, "apply").await?;
    }
    emit_text(emitter, &verification_markdown(&verified)).await?;
    Ok(verified)
}

async fn emit_verification_stages(
    emitter: &ChatEventEmitter,
    record: &ProposalRecord,
    report: &ChatEditVerificationReport,
) -> std::result::Result<(), RuntimeFailure> {
    if let Some(run_id) = &report.worktree_run_id {
        emitter
            .send(ChatProtocolEventPayload::WorktreeCreated {
                proposal_id: record.proposal_id.clone(),
                run_id: run_id.clone(),
            })
            .await
            .map_err(event_failure)?;
    }
    emitter
        .send(ChatProtocolEventPayload::EditAppliedInWorktree {
            proposal_id: record.proposal_id.clone(),
            success: report.apply.status == EditStageStatus::Passed,
        })
        .await
        .map_err(event_failure)?;
    if report.build.status != EditStageStatus::NotRun {
        emitter
            .send(ChatProtocolEventPayload::BuildStarted {
                proposal_id: record.proposal_id.clone(),
                offline: true,
            })
            .await
            .map_err(event_failure)?;
        emitter
            .send(ChatProtocolEventPayload::BuildCompleted {
                proposal_id: record.proposal_id.clone(),
                success: report.build.status == EditStageStatus::Passed
                    && report.tests.status != EditStageStatus::Failed,
                build: stage_name(report.build.status),
                tests: stage_name(report.tests.status),
            })
            .await
            .map_err(event_failure)?;
    }
    Ok(())
}

async fn emit_diff(
    emitter: &ChatEventEmitter,
    record: &ProposalRecord,
) -> std::result::Result<(), RuntimeFailure> {
    let diff = record
        .verified_diff
        .as_ref()
        .context("proposal has no verified diff")
        .map_err(|error| edit_failure("diff_review", "verified diff is unavailable", error))?;
    let mut changes = Vec::with_capacity(record.files.len());
    for file in &record.files {
        let stats = diff
            .files
            .iter()
            .find(|candidate| candidate.path == file.path)
            .with_context(|| format!("verified diff has no statistics for {}", file.path))
            .map_err(|error| {
                edit_failure(
                    "diff_review",
                    "verified diff statistics are incomplete",
                    error,
                )
            })?;
        changes.push(ChatEditReviewFile {
            path: file.path.clone(),
            status: match file.status {
                ProposalFileStatus::Modified => "modified",
                ProposalFileStatus::Created => "created",
            }
            .to_string(),
            line_ending: file.line_ending.as_str().to_string(),
            base_content: file.base_content.clone(),
            base_hash: file.base_hash.clone(),
            proposed_content: file.proposed_content.clone(),
            proposed_hash: file.proposed_hash.clone(),
            proposed_bytes: file.proposed_bytes,
            additions: stats.additions,
            deletions: stats.deletions,
            hunks: stats.hunks,
        });
    }
    emitter
        .send(ChatProtocolEventPayload::DiffReady {
            proposal_id: record.proposal_id.clone(),
            files: diff.statistics.files,
            additions: diff.statistics.additions,
            deletions: diff.statistics.deletions,
            display_patch: diff.display_patch.clone(),
            display_truncated: diff.display_truncated,
            changes,
        })
        .await
        .map_err(event_failure)
}

async fn emit_approval_required(
    emitter: &ChatEventEmitter,
    record: &ProposalRecord,
    operation: &str,
) -> std::result::Result<(), RuntimeFailure> {
    let diff = record
        .verified_diff
        .as_ref()
        .context("approval requires a verified diff")
        .map_err(|error| {
            edit_failure(
                "approval_precondition",
                "approval binding is incomplete",
                error,
            )
        })?;
    let summary = format!(
        "{} {} verified file(s), +{} / -{}, HEAD {}; offline build {}.",
        if operation == "rollback" {
            "Rollback"
        } else {
            "Apply"
        },
        diff.statistics.files,
        diff.statistics.additions,
        diff.statistics.deletions,
        record.plan.base_head,
        record
            .verification
            .as_ref()
            .map_or("unavailable", |report| stage_label(report.build.status)),
    );
    emitter
        .send(ChatProtocolEventPayload::ApprovalRequired {
            proposal_id: record.proposal_id.clone(),
            approval_request_id: approval_request_id(record, operation).map_err(|error| {
                edit_failure(
                    "approval_precondition",
                    "approval binding could not be created",
                    error,
                )
            })?,
            operation: operation.to_string(),
            summary,
        })
        .await
        .map_err(event_failure)
}

async fn emit_policy_records(
    emitter: &ChatEventEmitter,
    record: &ProposalRecord,
    offset: usize,
) -> std::result::Result<(), RuntimeFailure> {
    for policy in record.policy.iter().skip(offset) {
        emit_policy(emitter, Some(&record.proposal_id), policy).await?;
    }
    Ok(())
}

async fn emit_policy(
    emitter: &ChatEventEmitter,
    proposal_id: Option<&str>,
    policy: &PolicyDecisionRecord,
) -> std::result::Result<(), RuntimeFailure> {
    emitter
        .send(ChatProtocolEventPayload::PolicyDecision {
            proposal_id: proposal_id.map(str::to_string),
            stage: policy.stage.clone(),
            action_kind: policy.action_kind.clone(),
            decision: policy.decision.clone(),
            rule_id: policy.rule_id.clone(),
            audit_event_id: policy.audit_event_id.clone(),
        })
        .await
        .map_err(event_failure)
}

fn resolve_proposal(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    options: &ChatRuntimeOptions,
    transaction_preferred: bool,
) -> std::result::Result<(ProposalStore, ProposalRecord), RuntimeFailure> {
    let root_hash = canonical_root_hash(&prepared.workspace).map_err(|error| {
        RuntimeFailure::new(
            "workspace_identity",
            "proposal_store",
            format!("workspace identity could not be established: {error}"),
        )
    })?;
    let store = proposal_store(options, &root_hash).map_err(|error| {
        edit_failure(
            "proposal_store",
            "proposal store could not be opened",
            error,
        )
    })?;
    let explicit_proposal = request
        .edit
        .as_ref()
        .and_then(|edit| edit.proposal_id.as_deref())
        .or_else(|| {
            request
                .references
                .iter()
                .find_map(|reference| match &reference.target {
                    ChatReferenceTarget::Diff { proposal_id } => Some(proposal_id.as_str()),
                    _ => None,
                })
        });
    let explicit_transaction = request
        .edit
        .as_ref()
        .and_then(|edit| edit.transaction_id.as_deref());
    let prompt_id = bounded_identifier(request.prompt.trim());
    let record = (|| -> Result<ProposalRecord> {
        if let Some(proposal_id) = explicit_proposal {
            store.load(proposal_id)
        } else if let Some(transaction_id) = explicit_transaction {
            store
                .find_by_transaction_id(transaction_id)?
                .context("transaction is not known in this workspace")
        } else if transaction_preferred {
            if let Some(identifier) = prompt_id {
                match store.find_by_transaction_id(identifier)? {
                    Some(record) => Ok(record),
                    None => store.load(identifier),
                }
            } else {
                store
                    .latest()?
                    .context("no proposal exists in this workspace")
            }
        } else if let Some(identifier) = prompt_id {
            store.load(identifier)
        } else {
            store
                .latest()?
                .context("no proposal exists in this workspace")
        }
    })()
    .map_err(|error| {
        edit_failure(
            "proposal_resolution",
            "requested proposal could not be resolved",
            error,
        )
    })?;
    if record.workspace_id != request.workspace_id || record.workspace_root_hash != root_hash {
        return Err(RuntimeFailure::new(
            "proposal_workspace_mismatch",
            "proposal_resolution",
            "proposal belongs to another workspace identity",
        ));
    }
    if let Some(transaction_id) = explicit_transaction {
        let bound = record
            .apply
            .as_ref()
            .map(|report| report.transaction_id.as_str());
        if bound != Some(transaction_id) {
            return Err(RuntimeFailure::new(
                "transaction_proposal_mismatch",
                "proposal_resolution",
                "transaction ID is not the exact transaction bound to this proposal",
            ));
        }
    }
    Ok((store, record))
}

fn ensure_reviewable(record: &ProposalRecord) -> std::result::Result<(), RuntimeFailure> {
    if record.expired(opticcode_edit::unix_millis())
        || matches!(
            record.state,
            ProposalState::Expired | ProposalState::Discarded | ProposalState::Failed
        )
        || record.verified_diff.is_none()
        || record.verification.is_none()
    {
        return Err(RuntimeFailure::new(
            "proposal_not_reviewable",
            "diff_review",
            "proposal is expired, discarded, failed, or has no successful verified diff",
        ));
    }
    Ok(())
}

fn validate_native_confirmation(
    request: &ChatRequest,
    confirmation: &crate::ChatNativeConfirmation,
    expected_approval_request_id: &str,
) -> std::result::Result<(), RuntimeFailure> {
    if request.client.name != "opticcode-vscode"
        || confirmation.client != request.client.name
        || confirmation.approval_request_id != expected_approval_request_id
    {
        return Err(RuntimeFailure::new(
            "native_confirmation_mismatch",
            "native_confirmation",
            "confirmation is not bound to this VS Code client, proposal, and operation",
        ));
    }
    Ok(())
}

fn approval_request_id(record: &ProposalRecord, operation: &str) -> Result<String> {
    let binding = if operation == "rollback" {
        record
            .apply
            .as_ref()
            .map(|report| report.transaction_id.as_str())
            .context("rollback approval has no transaction binding")?
    } else {
        record
            .verified_diff
            .as_ref()
            .map(|diff| diff.patch_hash.as_str())
            .context("apply approval has no verified diff binding")?
    };
    let digest = blake3::hash(
        format!(
            "chat-edit-confirmation-v1\0{}\0{}\0{}",
            operation, record.proposal_id, binding
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    Ok(format!("{operation}-confirmation-{}", &digest[..32]))
}

fn edit_intent_selection_mode_name(mode: EditIntentSelectionMode) -> &'static str {
    match mode {
        EditIntentSelectionMode::ExplicitReferences => "explicit_references",
        EditIntentSelectionMode::ResolvedContext => "resolved_context",
        EditIntentSelectionMode::Hybrid => "hybrid",
    }
}

fn build_runtime_edit_intent(
    request: &ChatRequest,
    expected: &EditPlanExpectations,
    trusted_files: &[TrustedEditFile],
    references: &[crate::ChatResolvedReference],
    intent_id: String,
) -> Result<ValidatedEditIntent> {
    if trusted_files.is_empty() {
        anyhow::bail!("edit intent requires at least one resolved, trusted file reference");
    }

    let mut allowed_existing_targets = Vec::with_capacity(trusted_files.len());
    let mut targets = Vec::with_capacity(trusted_files.len());
    for file in trusted_files {
        let mut reference_ids = references
            .iter()
            .filter(|reference| reference.path.as_deref() == Some(file.path.as_str()))
            .map(|reference| reference.reference_id.clone())
            .collect::<Vec<_>>();
        reference_ids.sort();
        reference_ids.dedup();
        if reference_ids.is_empty() {
            anyhow::bail!(
                "trusted edit file {} has no bound resolved reference identity",
                file.path
            );
        }

        allowed_existing_targets.push(EditIntentAllowedExistingTarget {
            path: file.path.clone(),
            content_hash: file.content_hash.clone(),
            reference_ids: reference_ids.clone(),
        });
        targets.push(EditIntentTarget::ExistingFile {
            path: file.path.clone(),
            content_hash: file.content_hash.clone(),
            reference_ids,
            provenance: EditIntentTargetProvenance::UserReference,
        });
    }

    let now = expected.now_unix_ms;
    let constraints = EditIntentConstraints::modify_only(expected.limits);
    let expectations = EditIntentExpectations {
        request_id: expected.request_id.clone(),
        workspace_id: expected.workspace_id.clone(),
        workspace_root_hash: expected.workspace_root_hash.clone(),
        base_head: expected.base_head.clone(),
        working_tree_digest: expected.working_tree_digest.clone(),
        now_unix_ms: now,
        limits: expected.limits,
        allowed_existing_targets,
        allowed_create_targets: Vec::new(),
    };
    let intent = EditIntent {
        schema_version: EDIT_INTENT_SCHEMA_VERSION,
        intent_id,
        request_id: expected.request_id.clone(),
        workspace_id: expected.workspace_id.clone(),
        workspace_root_hash: expected.workspace_root_hash.clone(),
        base_head: expected.base_head.clone(),
        working_tree_digest: expected.working_tree_digest.clone(),
        task: request.prompt.clone(),
        selection_mode: EditIntentSelectionMode::ExplicitReferences,
        targets,
        constraints,
        created_at_unix_ms: now,
        expires_at_unix_ms: now
            .saturating_add(DEFAULT_EDIT_INTENT_TTL_SECONDS.saturating_mul(1_000)),
    };

    validate_edit_intent(intent, &expectations)
        .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))
}
fn available_edit_files(root: &Path) -> Result<Vec<String>> {
    let report = inspect_workspace(root)?;
    let mut files = report
        .sampled_files
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(root).ok()?;
            let extension = relative.extension()?.to_str()?.to_ascii_lowercase();
            ALLOWED_EDIT_EXTENSIONS
                .contains(&extension.as_str())
                .then(|| relative.to_string_lossy().replace('\\', "/"))
        })
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files.truncate(2_000);
    Ok(files)
}

fn trusted_edit_files(
    root: &Path,
    references: &[crate::ChatResolvedReference],
) -> Result<Vec<TrustedEditFile>> {
    let mut paths = references
        .iter()
        .filter_map(|reference| reference.path.as_deref())
        .map(str::to_string)
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    if paths.len() > opticcode_edit::MAX_EDIT_FILES {
        anyhow::bail!(
            "resolved edit references exceed the {} file runtime limit",
            opticcode_edit::MAX_EDIT_FILES
        );
    }

    paths
        .into_iter()
        .map(|path| {
            let file = read_safe_workspace_file(root, Path::new(&path), MAX_EDIT_FILE_BYTES as u64)
                .with_context(|| format!("failed to read trusted edit file {path}"))?;
            let line_ending = opticcode_edit::detect_line_ending(&file.content)
                .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))?;
            Ok(TrustedEditFile {
                path: file.relative_path,
                content_hash: opticcode_edit::content_hash(file.content.as_bytes()),
                bytes: file.content.len(),
                line_ending,
                line_anchors: trusted_line_anchors(&file.content),
            })
        })
        .collect()
}

fn trusted_line_anchors(content: &str) -> Vec<TrustedEditLine> {
    let mut start = 0usize;
    content
        .split_inclusive('\n')
        .map(|line| {
            let end = start.saturating_add(line.len());
            let anchor = TrustedEditLine {
                start,
                end,
                content: line.to_string(),
            };
            start = end;
            anchor
        })
        .collect()
}

fn proposal_store(options: &ChatRuntimeOptions, workspace_hash: &str) -> Result<ProposalStore> {
    match &options.proposal_state_root {
        Some(root) => ProposalStore::open(root, workspace_hash),
        None => ProposalStore::default_store(workspace_hash),
    }
}

fn open_policy(state_root: Option<PathBuf>) -> Result<PolicyEngine> {
    match state_root {
        Some(root) => Ok(PolicyEngine::open(root)?),
        None => Ok(PolicyEngine::default_engine()?),
    }
}

fn edit_options(request: &ChatRequest, prepared: &PreparedRequest) -> EditRuntimeOptions {
    let mut options = EditRuntimeOptions::new(
        prepared.workspace.clone(),
        request.workspace_id.clone(),
        request.request_id.clone(),
        request.profile.clone(),
    );
    options.client_name = request.client.name.clone();
    options.client_version = request.client.version.clone();
    options
}

async fn run_blocking_edit<T, F>(
    cancellation: &opticcode_llm::CancellationToken,
    stage: &'static str,
    operation: F,
) -> std::result::Result<T, RuntimeFailure>
where
    T: Send + 'static,
    F: FnOnce(&ToolCancellationToken) -> Result<T> + Send + 'static,
{
    let tool_cancellation = ToolCancellationToken::new();
    let monitor_token = tool_cancellation.clone();
    let request_cancellation = cancellation.clone();
    let monitor = tokio::spawn(async move {
        request_cancellation.cancelled().await;
        monitor_token.cancel();
    });
    let worker_token = tool_cancellation.clone();
    let result = tokio::task::spawn_blocking(move || operation(&worker_token)).await;
    monitor.abort();
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(edit_failure(stage, "bounded edit operation failed", error)),
        Err(error) => Err(RuntimeFailure::new(
            "edit_worker_failed",
            stage,
            format!("bounded edit worker could not complete: {error}"),
        )),
    }
}

fn outcome_from_generation(
    context_mode: ContextMode,
    generation: &GenerationResult,
    record: &ProposalRecord,
) -> CommandOutcome {
    let seconds = generation.timings.generation_ms.unwrap_or(0) as f64 / 1_000.0;
    let tokens_per_second = generation
        .usage
        .generated_tokens
        .filter(|_| seconds > 0.0)
        .map(|tokens| tokens as f64 / seconds);
    CommandOutcome {
        context_files: Vec::new(),
        used_context_mode: Some(context_mode),
        metrics: ChatMetrics {
            preparation_ms: 0,
            total_ms: generation.timings.client_ms,
            estimated_prompt_tokens: generation.prompt_chars.div_ceil(4),
            prompt_tokens: generation.usage.prompt_tokens,
            generated_tokens: generation.usage.generated_tokens,
            generated_tokens_per_second: tokens_per_second,
            timing: None,
            route: GroundingRoute::AutomaticAssistant.as_str().to_string(),
        },
        warnings: verification_warning(record).into_iter().collect(),
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    }
}

fn edit_outcome(used_context_mode: Option<ContextMode>, warnings: Vec<String>) -> CommandOutcome {
    CommandOutcome {
        context_files: Vec::new(),
        used_context_mode,
        metrics: ChatMetrics::default(),
        warnings,
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    }
}

fn verification_warning(record: &ProposalRecord) -> Option<String> {
    record
        .verification
        .as_ref()
        .filter(|report| !report.success)
        .map(|_| "proposal verification failed closed; apply is unavailable".to_string())
}

fn verification_markdown(record: &ProposalRecord) -> String {
    let Some(report) = &record.verification else {
        return format!(
            "Proposal `{}` has no verification report.",
            record.proposal_id
        );
    };
    if report.success {
        let diff = record.verified_diff.as_ref();
        format!(
            "Proposal `{}` is **verified** in an isolated worktree: {} file(s), +{} / -{}. The original workspace remained unchanged.",
            record.proposal_id,
            diff.map_or(0, |value| value.statistics.files),
            diff.map_or(0, |value| value.statistics.additions),
            diff.map_or(0, |value| value.statistics.deletions),
        )
    } else {
        format!(
            "Proposal `{}` is **verification_failed**. Build: {}; cleanup: {}; the Apply action is unavailable.",
            record.proposal_id,
            stage_label(report.build.status),
            stage_label(report.cleanup.status),
        )
    }
}

fn diff_markdown(record: &ProposalRecord) -> String {
    let Some(diff) = &record.verified_diff else {
        return format!("Proposal `{}` has no verified diff.", record.proposal_id);
    };
    let mut lines = vec![format!(
        "**Verified diff `{}`:** {} file(s), +{} / -{}.",
        record.proposal_id,
        diff.statistics.files,
        diff.statistics.additions,
        diff.statistics.deletions
    )];
    for file in &diff.files {
        lines.push(format!(
            "- `{}` +{} / -{} ({:?})",
            file.path, file.additions, file.deletions, file.status
        ));
    }
    lines.join("\n")
}

fn stage_name(status: EditStageStatus) -> String {
    stage_label(status).to_string()
}

const fn stage_label(status: EditStageStatus) -> &'static str {
    match status {
        EditStageStatus::NotRun => "not_run",
        EditStageStatus::Running => "running",
        EditStageStatus::Passed => "passed",
        EditStageStatus::Failed => "failed",
        EditStageStatus::Cancelled => "cancelled",
        EditStageStatus::Unavailable => "unavailable",
    }
}

fn state_name(state: ProposalState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn bounded_identifier(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)))
    .then_some(value)
}

fn event_failure(error: anyhow::Error) -> RuntimeFailure {
    RuntimeFailure::new("chat_event_delivery", "event_delivery", error.to_string())
}

fn edit_failure(stage: &'static str, summary: &str, error: anyhow::Error) -> RuntimeFailure {
    RuntimeFailure::new("chat_edit_failed", stage, format!("{summary}: {error:#}"))
}
