use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::Result;
use opticcode_policy::{
    ActionOrigin, ContextAction, GitReadAction, GitReadOperation, PolicyAction, PolicyClient,
    PolicyEngine, PolicyMode, PolicyRequest, PolicyWorkspace,
};
use opticcode_tools::git_state::capture_git_state;
use opticcode_tools::inspect_workspace;
use opticcode_tools::java_edits::{propose_java_edits, JavaEditOptions};
use opticcode_tools::java_index::{analyze_java_index, JavaIndexOptions};
use opticcode_tools::java_syntax::{analyze_java_syntax, JavaSyntaxOptions};
use opticcode_tools::rag::{
    inspect_sensitive_text, read_safe_workspace_file, MAX_SAFE_REFERENCE_FILE_BYTES,
};

use crate::chat_protocol::{
    ChatCommand, ChatCompletionSummary, ChatContextFile, ChatEventEmitter, ChatGroundingSummary,
    ChatMetrics, ChatProtocolError, ChatProtocolEventPayload, ChatProtocolSession, ChatReference,
    ChatReferenceTarget, ChatRejectedReference, ChatRequest, ChatResolvedReference,
    ChatSecurityMode, ChatTextPosition, ChatTextRange, ChatTimingPhase, ChatTimingReport,
    CHAT_PROTOCOL_ID, CHAT_PROTOCOL_SCHEMA_VERSION, MAX_CHAT_EVENT_TEXT_BYTES,
    MAX_CHAT_HISTORY_CHARS, MAX_CHAT_HISTORY_TOKENS, MAX_CHAT_HISTORY_TURNS,
    MAX_CHAT_OUTPUT_TOKENS, MAX_CHAT_PROMPT_CHARS, MAX_CHAT_REFERENCES, MAX_CHAT_REFERENCE_BYTES,
};
use crate::grounding::{
    build_context_manifest, build_grounded_prompt, effective_context_scope,
    grounded_response_schema, inspect_document_facts, prompt_fingerprint, validate_compliance,
    validate_evidence, PromptFingerprintInput, ReferenceSnapshot,
};
use crate::{
    assistant_event_channel, prepare_assistant_context, AskOptions, AssistantCommandReport,
    AssistantProtocolEventPayload, AssistantProtocolSession, ChatContextScope, ChatEvidenceMode,
    ChatScopeReason, ComplianceReport, ContextFallbackPolicy, ContextManifest,
    ContextManifestRange, ContextMode, EvidenceValidationReport, GenerationResult,
    GroundedResponse, GroundingRoute, OpticCode, PlanOptions, ASSISTANT_PROTOCOL_SCHEMA_VERSION,
    DEFAULT_ASSISTANT_EVENT_CAPACITY,
};

const MAX_ID_BYTES: usize = 128;
const MAX_MODEL_BYTES: usize = 256;
const MAX_PROFILE_BYTES: usize = 96;
const MAX_HISTORY_TURN_CHARS: usize = 8 * 1024;
const MAX_REFERENCE_REASON_CHARS: usize = 512;
const MAX_SYMBOL_CHARS: usize = 256;
const MAX_WARNING_COUNT: usize = 64;
const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct ChatRuntimeOptions {
    pub rag_index: PathBuf,
    pub verify_model: bool,
    pub policy_state_root: Option<PathBuf>,
    pub proposal_state_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatExecutionStatus {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ChatExecutionReport {
    pub request_id: String,
    pub status: ChatExecutionStatus,
    pub repository_state: String,
}

#[derive(Debug)]
pub(crate) struct RuntimeFailure {
    code: &'static str,
    stage: &'static str,
    message: String,
    retriable: bool,
}

impl RuntimeFailure {
    pub(crate) fn new(code: &'static str, stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            stage,
            message: bounded_text(&message.into(), 8 * 1024),
            retriable: false,
        }
    }

    pub(crate) fn retriable(mut self, retriable: bool) -> Self {
        self.retriable = retriable;
        self
    }

    fn protocol_error(&self) -> ChatProtocolError {
        ChatProtocolError {
            code: self.code.to_string(),
            stage: self.stage.to_string(),
            message: self.message.clone(),
            retriable: self.retriable,
        }
    }
}

impl std::fmt::Display for RuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} at {}: {}",
            self.code, self.stage, self.message
        )
    }
}

impl std::error::Error for RuntimeFailure {}

#[derive(Debug)]
pub(crate) struct PreparedRequest {
    pub(crate) workspace: PathBuf,
    pub(crate) prompt: String,
    pub(crate) user_prompt: String,
    pub(crate) references: Vec<ChatResolvedReference>,
    pub(crate) rejected: Vec<ChatRejectedReference>,
    pub(crate) warnings: Vec<String>,
    pub(crate) repository_state: String,
    pub(crate) requested_scope: ChatContextScope,
    pub(crate) effective_scope: ChatContextScope,
    pub(crate) scope_reason: ChatScopeReason,
    pub(crate) evidence_mode: ChatEvidenceMode,
    pub(crate) manifest: ContextManifest,
    pub(crate) prompt_fingerprint: String,
    pub(crate) snapshots: Vec<ReferenceSnapshot>,
    pub(crate) selected_references: usize,
    pub(crate) resolved_references: usize,
    pub(crate) historical_turns: usize,
    pub(crate) reference_resolution_ms: u64,
    pub(crate) prompt_build_ms: u64,
}

#[derive(Debug)]
struct ReferenceMaterial {
    summary: ChatResolvedReference,
    prompt_content: String,
    snapshot: Option<ReferenceSnapshot>,
}

#[derive(Debug)]
pub(crate) struct CommandOutcome {
    pub(crate) context_files: Vec<ChatContextFile>,
    pub(crate) used_context_mode: Option<ContextMode>,
    pub(crate) metrics: ChatMetrics,
    pub(crate) warnings: Vec<String>,
    pub(crate) route: GroundingRoute,
    pub(crate) grounding_response: Option<GroundedResponse>,
    pub(crate) evidence: Option<EvidenceValidationReport>,
    pub(crate) compliance: Option<ComplianceReport>,
    pub(crate) rag_hits: usize,
}

#[derive(Debug)]
pub(crate) struct ChatPolicyAuthorization {
    pub(crate) policy_version: String,
    pub(crate) decision: String,
    pub(crate) rule_id: String,
    pub(crate) action_kind: String,
    pub(crate) action_hash: String,
    pub(crate) audit_event_id: Option<String>,
    pub(crate) effective_security_mode: ChatSecurityMode,
}

pub async fn execute_chat(
    app: Option<&OpticCode>,
    request: ChatRequest,
    session: ChatProtocolSession,
    options: ChatRuntimeOptions,
) -> Result<ChatExecutionReport> {
    let request_id = session.request_id.clone();
    let cancellation = session.cancellation.clone();
    let emitter = ChatEventEmitter::new(&session)?;
    let started = Instant::now();
    let result =
        execute_chat_inner(app, &request, &emitter, &cancellation, &options, started).await;

    match result {
        Ok((prepared, _outcome)) if cancellation.is_cancelled() => {
            emitter
                .send(ChatProtocolEventPayload::Cancelled {
                    reason: "request cancellation was confirmed".to_string(),
                })
                .await?;
            Ok(ChatExecutionReport {
                request_id,
                status: ChatExecutionStatus::Cancelled,
                repository_state: prepared.repository_state,
            })
        }
        Ok((prepared, mut outcome)) => {
            outcome.metrics.total_ms = elapsed_ms(started);
            outcome.metrics.route = outcome.route.as_str().to_string();
            let timing = outcome
                .metrics
                .timing
                .get_or_insert_with(|| timing_report(&request, Vec::new()));
            timing.phases.retain(|phase| phase.name != "runtime_total");
            timing.phases.push(timing_phase(
                "runtime_total",
                outcome.metrics.total_ms,
                "opticcode-core",
                &["request_received", "terminal_preparation"],
            ));
            emitter
                .send(ChatProtocolEventPayload::Metrics {
                    metrics: outcome.metrics.clone(),
                })
                .await?;
            emitter
                .send(ChatProtocolEventPayload::TimingMetrics {
                    metrics: outcome.metrics.clone(),
                })
                .await?;
            let mut warnings = prepared.warnings.clone();
            warnings.extend(outcome.warnings);
            warnings.truncate(MAX_WARNING_COUNT);
            let grounding = ChatGroundingSummary {
                schema_version: crate::GROUNDING_SCHEMA_VERSION,
                route: outcome.route,
                requested_scope: prepared.requested_scope,
                effective_scope: prepared.effective_scope,
                scope_reason: prepared.scope_reason,
                evidence_mode: prepared.evidence_mode,
                selected_references: prepared.selected_references,
                resolved_references: prepared.resolved_references,
                injected_references: prepared.manifest.entries.len(),
                refused_references: prepared.rejected.len(),
                discovered_files: if outcome.route == GroundingRoute::AutomaticAssistant {
                    outcome.context_files.len()
                } else {
                    0
                },
                rag_hits: outcome.rag_hits,
                historical_turns: prepared.historical_turns,
                prompt_fingerprint: prepared.prompt_fingerprint.clone(),
                manifest: prepared.manifest.clone(),
                response: outcome.grounding_response.clone(),
                evidence: outcome.evidence.clone(),
                compliance: outcome.compliance.clone(),
            };
            let summary = ChatCompletionSummary {
                command: request.command,
                success: true,
                model: request.model.clone(),
                requested_context_mode: request.context_mode,
                used_context_mode: outcome.used_context_mode,
                references: prepared.references.clone(),
                rejected_references: prepared.rejected.len(),
                context_files: outcome.context_files,
                warnings,
                metrics: outcome.metrics,
                repository_state: prepared.repository_state.clone(),
                run_id: request.request_id.clone(),
                grounding: Some(grounding),
            };
            emitter
                .send(ChatProtocolEventPayload::Completed {
                    summary: Box::new(summary),
                })
                .await?;
            Ok(ChatExecutionReport {
                request_id,
                status: ChatExecutionStatus::Completed,
                repository_state: prepared.repository_state,
            })
        }
        Err(failure) if cancellation.is_cancelled() => {
            emitter
                .send(ChatProtocolEventPayload::Cancelled {
                    reason: bounded_text(&failure.message, 4 * 1024),
                })
                .await?;
            Ok(ChatExecutionReport {
                request_id,
                status: ChatExecutionStatus::Cancelled,
                repository_state: "unavailable".to_string(),
            })
        }
        Err(failure) => {
            emitter
                .send(ChatProtocolEventPayload::Failed {
                    error: failure.protocol_error(),
                })
                .await?;
            Ok(ChatExecutionReport {
                request_id,
                status: ChatExecutionStatus::Failed,
                repository_state: "unavailable".to_string(),
            })
        }
    }
}

async fn execute_chat_inner(
    app: Option<&OpticCode>,
    request: &ChatRequest,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
    started: Instant,
) -> std::result::Result<(PreparedRequest, CommandOutcome), RuntimeFailure> {
    validate_request(request)?;
    let authorization = authorize_chat_request(request, options)?;
    emitter
        .send(ChatProtocolEventPayload::RequestAccepted {
            command: request.command,
            requested_security_mode: request.security_mode,
            security_mode: authorization.effective_security_mode,
            effective_security_mode: authorization.effective_security_mode,
            policy_version: authorization.policy_version.clone(),
            policy_decision: authorization.decision.clone(),
            policy_rule_id: authorization.rule_id.clone(),
        })
        .await
        .map_err(event_failure)?;
    if request.security_mode != ChatSecurityMode::ReadOnly {
        return Err(RuntimeFailure::new(
            "security_mode_unavailable",
            "policy",
            "clients must request read_only; only the Rust runtime may perform scoped worktree_edit or approved_apply transitions",
        ));
    }
    if cancellation.is_cancelled() {
        return Err(RuntimeFailure::new(
            "request_cancelled",
            "request_acceptance",
            "request was cancelled before reference resolution",
        ));
    }

    let prepared = prepare_request(request, emitter).await?;
    if cancellation.is_cancelled() {
        return Ok((prepared, empty_outcome(started)));
    }

    let outcome = match request.command {
        ChatCommand::Ask | ChatCommand::Plan => {
            if prepared.effective_scope != ChatContextScope::Automatic
                && !prepared.snapshots.is_empty()
            {
                run_grounded_assistant(
                    app,
                    request,
                    &prepared,
                    emitter,
                    cancellation,
                    options,
                    started,
                )
                .await?
            } else {
                let app = app.ok_or_else(|| {
                    RuntimeFailure::new(
                        "provider_unavailable",
                        "provider_setup",
                        "the configured local LLM provider is unavailable",
                    )
                    .retriable(true)
                })?;
                run_assistant(
                    app,
                    request,
                    &prepared,
                    emitter,
                    cancellation,
                    options,
                    started,
                )
                .await?
            }
        }
        ChatCommand::Inspect => {
            run_grounded_assistant(
                app,
                request,
                &prepared,
                emitter,
                cancellation,
                options,
                started,
            )
            .await?
        }
        ChatCommand::Context => run_context_command(request, &prepared, emitter, started).await?,
        ChatCommand::Analyze => run_analyze_command(request, &prepared, emitter, started).await?,
        ChatCommand::Index => run_index_command(request, &prepared, emitter, started).await?,
        ChatCommand::Legacy => run_legacy_command(request, &prepared, emitter, started).await?,
        ChatCommand::Status => run_status_command(request, &prepared, emitter, started).await?,
        ChatCommand::Runs => run_runs_command(request, emitter, started).await?,
        ChatCommand::Help => run_help_command(emitter, started).await?,
        ChatCommand::Fix
        | ChatCommand::Verify
        | ChatCommand::Diff
        | ChatCommand::Apply
        | ChatCommand::Rollback => {
            crate::chat_edit_runtime::run_chat_edit(
                app,
                request,
                &prepared,
                &authorization,
                emitter,
                cancellation,
                options,
                started,
            )
            .await?
        }
        ChatCommand::Unknown => {
            return Err(RuntimeFailure::new(
                "unknown_command",
                "request_validation",
                "unknown OpticCode chat slash command",
            ));
        }
    };
    Ok((prepared, outcome))
}

fn authorize_chat_request(
    request: &ChatRequest,
    options: &ChatRuntimeOptions,
) -> std::result::Result<ChatPolicyAuthorization, RuntimeFailure> {
    let engine = match options.policy_state_root.as_ref() {
        Some(root) => PolicyEngine::open(root),
        None => PolicyEngine::default_engine(),
    }
    .map_err(|error| {
        RuntimeFailure::new(
            "policy_unavailable",
            "policy",
            format!("deny-by-default policy runtime is unavailable: {error}"),
        )
    })?;
    let workspace_root = PathBuf::from(&request.workspace_root);
    let action = if request.command == ChatCommand::Status {
        PolicyAction::GitRead(GitReadAction {
            repository_root: workspace_root.clone(),
            operation: GitReadOperation::Status,
            paths: Vec::new(),
        })
    } else {
        PolicyAction::BuildContext(ContextAction {
            root: workspace_root.clone(),
            task_hash: blake3::hash(
                format!("{}:{}", request.command.as_str(), request.prompt).as_bytes(),
            )
            .to_hex()
            .to_string(),
            candidate_paths: Vec::new(),
        })
    };
    let policy_request = PolicyRequest {
        schema_version: opticcode_policy::POLICY_SCHEMA_VERSION,
        protocol: opticcode_policy::POLICY_PROTOCOL_ID.to_string(),
        request_id: request.request_id.clone(),
        action_id: format!("{}:chat_context", request.request_id),
        origin: ActionOrigin::Chat,
        profile: request.profile.clone(),
        client: PolicyClient {
            name: request.client.name.clone(),
            version: request.client.version.clone(),
        },
        mode: PolicyMode::ReadOnly,
        workspace: PolicyWorkspace {
            workspace_id: request.workspace_id.clone(),
            root: workspace_root,
            repository: None,
            active_worktree: None,
            working_tree_digest: None,
            repository_clean: None,
        },
        action,
        approval_id: None,
    };
    let preflight = engine.check(&policy_request).map_err(|error| {
        RuntimeFailure::new(
            "policy_check_failed",
            "policy",
            format!("policy could not authorize chat context: {error}"),
        )
    })?;
    if !preflight.report.allowed() {
        return Err(RuntimeFailure::new(
            "policy_denied",
            "policy",
            format!(
                "{}: {}",
                preflight.report.decision.rule_id(),
                preflight.report.user_reason
            ),
        ));
    }
    preflight.revalidate().map_err(|error| {
        RuntimeFailure::new(
            "policy_revalidation_failed",
            "policy",
            format!("chat context changed after policy authorization: {error}"),
        )
    })?;
    Ok(ChatPolicyAuthorization {
        policy_version: preflight.report.policy_version,
        decision: preflight.report.decision.kind().to_string(),
        rule_id: preflight.report.decision.rule_id().to_string(),
        action_kind: preflight.report.action_kind,
        action_hash: preflight.report.action_hash,
        audit_event_id: preflight.report.audit_event_id,
        effective_security_mode: ChatSecurityMode::ReadOnly,
    })
}

fn validate_request(request: &ChatRequest) -> std::result::Result<(), RuntimeFailure> {
    if request.schema_version != CHAT_PROTOCOL_SCHEMA_VERSION
        || request.protocol != CHAT_PROTOCOL_ID
    {
        return Err(RuntimeFailure::new(
            "protocol_incompatible",
            "request_validation",
            format!("expected {CHAT_PROTOCOL_ID} schema {CHAT_PROTOCOL_SCHEMA_VERSION}"),
        ));
    }
    crate::validate_chat_request_id(&request.request_id).map_err(|error| {
        RuntimeFailure::new(
            "invalid_request_id",
            "request_validation",
            error.to_string(),
        )
    })?;
    validate_identifier("workspace_id", &request.workspace_id, MAX_ID_BYTES)?;
    validate_identifier("session_id", &request.client.session_id, MAX_ID_BYTES)?;
    if request.command == ChatCommand::Unknown {
        return Err(RuntimeFailure::new(
            "unknown_command",
            "request_validation",
            "the requested chat command is not supported",
        ));
    }
    if request.expected_protocols.chat != CHAT_PROTOCOL_SCHEMA_VERSION
        || request.expected_protocols.assistant != ASSISTANT_PROTOCOL_SCHEMA_VERSION
        || request.expected_protocols.discovery != 1
        || request.expected_protocols.llm != 1
    {
        return Err(RuntimeFailure::new(
            "protocol_incompatible",
            "request_validation",
            "client protocol expectations do not match this OpticCode build",
        ));
    }
    if request.prompt.chars().count() > MAX_CHAT_PROMPT_CHARS {
        return Err(RuntimeFailure::new(
            "prompt_too_large",
            "request_validation",
            format!("chat prompt exceeds {MAX_CHAT_PROMPT_CHARS} characters"),
        ));
    }
    if request.command.requires_prompt() && request.prompt.trim().is_empty() {
        return Err(RuntimeFailure::new(
            "prompt_required",
            "request_validation",
            format!("/{} requires a non-empty prompt", request.command),
        ));
    }
    if request.model.trim().is_empty() || request.model.len() > MAX_MODEL_BYTES {
        return Err(RuntimeFailure::new(
            "invalid_model",
            "request_validation",
            "model identifier is empty or too long",
        ));
    }
    if request.profile.is_empty()
        || request.profile.len() > MAX_PROFILE_BYTES
        || !request
            .profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Err(RuntimeFailure::new(
            "invalid_profile",
            "request_validation",
            "profile identifier is invalid",
        ));
    }
    validate_budgets(request)?;
    if request.generation.max_output_tokens == 0
        || request.generation.max_output_tokens > MAX_CHAT_OUTPUT_TOKENS
    {
        return Err(RuntimeFailure::new(
            "invalid_generation_options",
            "request_validation",
            format!("max_output_tokens must be between 1 and {MAX_CHAT_OUTPUT_TOKENS}"),
        ));
    }
    if request
        .generation
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        return Err(RuntimeFailure::new(
            "invalid_generation_options",
            "request_validation",
            "temperature must be finite and between 0 and 2",
        ));
    }
    if request.generation.compare_generate && request.context_mode != ContextMode::Compare {
        return Err(RuntimeFailure::new(
            "invalid_generation_options",
            "request_validation",
            "compare_generate is valid only with compare context mode",
        ));
    }
    if let Some(edit) = &request.edit {
        if !matches!(
            request.command,
            ChatCommand::Fix
                | ChatCommand::Verify
                | ChatCommand::Diff
                | ChatCommand::Apply
                | ChatCommand::Rollback
        ) {
            return Err(RuntimeFailure::new(
                "invalid_edit_control",
                "request_validation",
                "structured edit controls are valid only for chat edit commands",
            ));
        }
        if let Some(proposal_id) = &edit.proposal_id {
            validate_identifier("proposal_id", proposal_id, 160)?;
        }
        if let Some(transaction_id) = &edit.transaction_id {
            validate_identifier("transaction_id", transaction_id, 160)?;
        }
        if edit.discard && request.command != ChatCommand::Diff {
            return Err(RuntimeFailure::new(
                "invalid_edit_control",
                "request_validation",
                "proposal discard is accepted only through the read-only diff command",
            ));
        }
        if let Some(confirmation) = &edit.native_confirmation {
            if !matches!(request.command, ChatCommand::Apply | ChatCommand::Rollback) {
                return Err(RuntimeFailure::new(
                    "invalid_native_confirmation",
                    "request_validation",
                    "native confirmation is accepted only for apply or rollback",
                ));
            }
            validate_identifier("confirmation.client", &confirmation.client, 96)?;
            validate_identifier(
                "confirmation.confirmation_id",
                &confirmation.confirmation_id,
                160,
            )?;
            validate_identifier(
                "confirmation.approval_request_id",
                &confirmation.approval_request_id,
                160,
            )?;
        }
    }
    let mut ids = BTreeSet::new();
    for reference in &request.references {
        validate_identifier("reference_id", &reference.reference_id, MAX_ID_BYTES)?;
        if !ids.insert(reference.reference_id.as_str()) {
            return Err(RuntimeFailure::new(
                "duplicate_reference",
                "request_validation",
                "reference IDs must be unique within a request",
            ));
        }
        if reference.inclusion_reason.chars().count() > MAX_REFERENCE_REASON_CHARS {
            return Err(RuntimeFailure::new(
                "reference_reason_too_large",
                "request_validation",
                "reference inclusion reason is too long",
            ));
        }
    }
    Ok(())
}

fn validate_identifier(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> std::result::Result<(), RuntimeFailure> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err(RuntimeFailure::new(
            "invalid_identifier",
            "request_validation",
            format!("{field} contains unsupported characters or exceeds its limit"),
        ));
    }
    Ok(())
}

fn validate_budgets(request: &ChatRequest) -> std::result::Result<(), RuntimeFailure> {
    let budgets = &request.budgets;
    let invalid = budgets.max_history_turns == 0
        || budgets.max_history_turns > MAX_CHAT_HISTORY_TURNS
        || budgets.max_history_chars == 0
        || budgets.max_history_chars > MAX_CHAT_HISTORY_CHARS
        || budgets.max_history_tokens == 0
        || budgets.max_history_tokens > MAX_CHAT_HISTORY_TOKENS
        || budgets.max_references == 0
        || budgets.max_references > MAX_CHAT_REFERENCES
        || budgets.max_reference_bytes == 0
        || budgets.max_reference_bytes > MAX_CHAT_REFERENCE_BYTES
        || budgets.max_prompt_tokens == 0
        || budgets.max_prompt_tokens > 64 * 1024
        || budgets.rag_hits > 12;
    if invalid || request.references.len() > budgets.max_references {
        return Err(RuntimeFailure::new(
            "invalid_budgets",
            "request_validation",
            "chat budgets exceed hard runtime limits",
        ));
    }
    Ok(())
}

async fn prepare_request(
    request: &ChatRequest,
    emitter: &ChatEventEmitter,
) -> std::result::Result<PreparedRequest, RuntimeFailure> {
    let workspace_input = PathBuf::from(&request.workspace_root);
    let metadata = std::fs::symlink_metadata(&workspace_input).map_err(|error| {
        RuntimeFailure::new(
            "workspace_unavailable",
            "workspace_validation",
            format!("workspace cannot be inspected: {error}"),
        )
    })?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        return Err(RuntimeFailure::new(
            "workspace_unsafe",
            "workspace_validation",
            "workspace must be a real directory, not a symlink or reparse point",
        ));
    }
    let workspace = std::fs::canonicalize(&workspace_input).map_err(|error| {
        RuntimeFailure::new(
            "workspace_unavailable",
            "workspace_validation",
            format!("workspace cannot be resolved: {error}"),
        )
    })?;
    let (effective_scope, scope_reason) =
        effective_context_scope(request.context_scope, request.scope_reason, &request.prompt);
    for reference in &request.references {
        emitter
            .send(ChatProtocolEventPayload::ReferenceSelected {
                reference_id: reference.reference_id.clone(),
                kind: reference.target.kind().to_string(),
                path: reference.target.path().map(ToString::to_string),
                origin: "user_attachment".to_string(),
            })
            .await
            .map_err(event_failure)?;
    }
    emitter
        .send(ChatProtocolEventPayload::ReferencesResolving {
            count: request.references.len(),
        })
        .await
        .map_err(event_failure)?;

    let resolution_started = Instant::now();
    let mut accepted_material = Vec::new();
    let mut rejected = Vec::new();
    let mut retained_bytes = 0usize;
    for reference in &request.references {
        match resolve_reference(
            &workspace,
            &request.workspace_id,
            reference,
            request.budgets.max_reference_bytes,
        ) {
            Ok(mut material)
                if retained_bytes.saturating_add(material.summary.bytes)
                    <= request.budgets.max_reference_bytes =>
            {
                retained_bytes = retained_bytes.saturating_add(material.summary.bytes);
                if material.snapshot.is_some() {
                    material.summary.injection = "injected".to_string();
                    material.summary.bytes_injected = material.summary.bytes;
                } else {
                    material.summary.injection = "identity_only".to_string();
                }
                emitter
                    .send(ChatProtocolEventPayload::ReferenceResolved {
                        reference: material.summary.clone(),
                    })
                    .await
                    .map_err(event_failure)?;
                accepted_material.push(material);
            }
            Ok(_) => {
                let refused = rejected_reference(
                    reference,
                    "size.total_reference_budget",
                    "reference would exceed the total attachment budget",
                );
                emitter
                    .send(ChatProtocolEventPayload::ReferenceRefused {
                        reference: refused.clone(),
                    })
                    .await
                    .map_err(event_failure)?;
                rejected.push(refused);
            }
            Err(error) => {
                let refused = rejected_reference(reference, error.code, &error.message);
                emitter
                    .send(ChatProtocolEventPayload::ReferenceRefused {
                        reference: refused.clone(),
                    })
                    .await
                    .map_err(event_failure)?;
                rejected.push(refused);
            }
        }
    }
    let reference_resolution_ms = duration_ms_floor(resolution_started.elapsed());
    let resolved = accepted_material
        .iter()
        .map(|material| material.summary.clone())
        .collect::<Vec<_>>();
    emitter
        .send(ChatProtocolEventPayload::ReferencesResolved {
            accepted: resolved,
            rejected: rejected.clone(),
        })
        .await
        .map_err(event_failure)?;

    let snapshots = accepted_material
        .iter()
        .filter_map(|material| material.snapshot.clone())
        .collect::<Vec<_>>();
    let references = accepted_material
        .iter()
        .filter(|material| material.snapshot.is_some())
        .map(|material| material.summary.clone())
        .collect::<Vec<_>>();
    for reference in &references {
        emitter
            .send(ChatProtocolEventPayload::ReferenceInjected {
                reference: reference.clone(),
            })
            .await
            .map_err(event_failure)?;
    }
    if effective_scope == ChatContextScope::ReferencesOnly && snapshots.is_empty() {
        return Err(RuntimeFailure::new(
            if request.references.is_empty() {
                "references_required"
            } else {
                "reference_unavailable"
            },
            "grounding_scope",
            "references_only requires at least one readable reference attached to the current request",
        ));
    }

    let mut warnings = rejected
        .iter()
        .map(|item| format!("{}: {}", item.rule_id, item.reason))
        .collect::<Vec<_>>();
    let allow_broad_context = effective_scope == ChatContextScope::Automatic
        || (effective_scope == ChatContextScope::ReferencesPreferred && snapshots.is_empty());
    let (history, history_warnings, historical_turns) = if allow_broad_context {
        bounded_history(request, effective_scope)
    } else {
        ("none".to_string(), Vec::new(), 0)
    };
    warnings.extend(history_warnings);
    warnings.truncate(MAX_WARNING_COUNT);
    for warning in &warnings {
        emitter
            .send(ChatProtocolEventPayload::Warning {
                code: "context_omission".to_string(),
                message: warning.clone(),
            })
            .await
            .map_err(event_failure)?;
    }

    let (project_summary, repository_state) = project_summary(&workspace);
    let manifest = build_context_manifest(
        effective_scope,
        &request.workspace_id,
        &request.request_id,
        &request.profile,
        &snapshots,
    );
    let prompt_build_started = Instant::now();
    let prompt = if allow_broad_context {
        let explicit_references = render_reference_material(&accepted_material);
        format!(
            concat!(
                "[PROJECT_SUMMARY]\n{}\n\n",
                "[CHAT_HISTORY]\n{}\n\n",
                "[CURRENT_REQUEST]\n{}\n\n",
                "[EXPLICIT_REFERENCES]\n{}"
            ),
            project_summary, history, request.prompt, explicit_references
        )
    } else {
        build_grounded_prompt(
            &request.prompt,
            &manifest,
            &snapshots,
            request.evidence_mode,
        )
        .map_err(|error| {
            RuntimeFailure::new(
                "grounded_prompt_failed",
                "prompt_preparation",
                error.to_string(),
            )
        })?
    };
    let prompt_build_ms = duration_ms_floor(prompt_build_started.elapsed());
    if estimate_tokens(&prompt) > request.budgets.max_prompt_tokens {
        return Err(RuntimeFailure::new(
            "prompt_budget_exceeded",
            "prompt_preparation",
            "bounded history and explicit references exceed the configured prompt budget",
        ));
    }
    let context_mode = request.context_mode.to_string();
    let provider = request.provider.to_string();
    let max_output_tokens = request.generation.max_output_tokens.to_string();
    let brief = request.generation.brief.to_string();
    let compare_generate = request.generation.compare_generate.to_string();
    let rag_hits = request.budgets.rag_hits.to_string();
    let prompt_fingerprint = prompt_fingerprint(PromptFingerprintInput {
        task: &request.prompt,
        manifest: &manifest,
        evidence_mode: request.evidence_mode,
        command: request.command.as_str(),
        model: &request.model,
        temperature: request.generation.temperature,
        seed: request.generation.seed,
        cache_dimensions: &[
            ("repository_state", repository_state.as_str()),
            ("context_mode", context_mode.as_str()),
            ("session_namespace", request.client.session_id.as_str()),
            ("provider", provider.as_str()),
            ("max_output_tokens", max_output_tokens.as_str()),
            ("brief", brief.as_str()),
            ("compare_generate", compare_generate.as_str()),
            ("rag_hits", rag_hits.as_str()),
            ("authorized_history", history.as_str()),
        ],
    });
    emitter
        .send(ChatProtocolEventPayload::ContextManifestReady {
            manifest: manifest.clone(),
            prompt_fingerprint: prompt_fingerprint.clone(),
        })
        .await
        .map_err(event_failure)?;
    Ok(PreparedRequest {
        workspace,
        prompt,
        user_prompt: request.prompt.clone(),
        references,
        rejected,
        warnings,
        repository_state,
        requested_scope: request.context_scope,
        effective_scope,
        scope_reason,
        evidence_mode: request.evidence_mode,
        manifest,
        prompt_fingerprint,
        snapshots,
        selected_references: request.references.len(),
        resolved_references: accepted_material.len(),
        historical_turns,
        reference_resolution_ms,
        prompt_build_ms,
    })
}

fn resolve_reference(
    workspace: &Path,
    workspace_id: &str,
    reference: &ChatReference,
    _total_budget: usize,
) -> std::result::Result<ReferenceMaterial, RuntimeFailure> {
    match &reference.target {
        ChatReferenceTarget::Run { run_id } => {
            validate_identifier("run_id", run_id, MAX_ID_BYTES)?;
            Ok(metadata_reference(
                reference,
                format!(
                    "Previous run `{run_id}` (summary only; report content was not reinjected)."
                ),
            ))
        }
        ChatReferenceTarget::Diff { proposal_id } => {
            validate_identifier("proposal_id", proposal_id, MAX_ID_BYTES)?;
            Ok(metadata_reference(
                reference,
                format!(
                    "Previous proposal `{proposal_id}` (identity only; large diff content was not reinjected)."
                ),
            ))
        }
        target => {
            let path = target.path().ok_or_else(|| {
                RuntimeFailure::new(
                    "reference.metadata_unavailable",
                    "reference_resolution",
                    "reference does not identify a readable workspace file",
                )
            })?;
            let file =
                read_safe_workspace_file(workspace, Path::new(path), MAX_SAFE_REFERENCE_FILE_BYTES)
                    .map_err(|error| {
                        RuntimeFailure::new(error.rule_id, "reference_resolution", error.message)
                    })?;
            let full_hash = blake3::hash(file.content.as_bytes()).to_hex().to_string();
            let (content, range) = match target {
                ChatReferenceTarget::Range { range, .. }
                | ChatReferenceTarget::Selection { range, .. } => {
                    extract_utf16_range_snapshot(&file.content, *range)?
                }
                ChatReferenceTarget::Symbol { symbol, range, .. } => {
                    if symbol.is_empty() || symbol.chars().count() > MAX_SYMBOL_CHARS {
                        return Err(RuntimeFailure::new(
                            "reference.invalid_symbol",
                            "reference_resolution",
                            "symbol identity is empty or too long",
                        ));
                    }
                    if let Some(range) = range {
                        extract_utf16_range_snapshot(&file.content, *range)?
                    } else {
                        let extracted = extract_symbol_context(&file.content, symbol)?;
                        let start = file.content.find(&extracted).ok_or_else(|| {
                            RuntimeFailure::new(
                                "reference.symbol_location",
                                "reference_resolution",
                                "materialized symbol could not be located in its source snapshot",
                            )
                        })?;
                        let end = start.saturating_add(extracted.len());
                        (
                            extracted,
                            manifest_range_from_bytes(&file.content, start, end),
                        )
                    }
                }
                ChatReferenceTarget::Finding {
                    range: Some(range), ..
                } => extract_utf16_range_snapshot(&file.content, *range)?,
                _ => {
                    let end = file.content.len();
                    (
                        file.content.clone(),
                        manifest_range_from_bytes(&file.content, 0, end),
                    )
                }
            };
            if content.len() > MAX_SAFE_REFERENCE_FILE_BYTES as usize {
                return Err(RuntimeFailure::new(
                    "size.reference_material",
                    "reference_resolution",
                    "materialized reference exceeds the hard byte limit",
                ));
            }
            let injected_hash = blake3::hash(content.as_bytes()).to_hex().to_string();
            let stable =
                read_safe_workspace_file(workspace, Path::new(path), MAX_SAFE_REFERENCE_FILE_BYTES)
                    .map_err(|error| {
                        RuntimeFailure::new(error.rule_id, "reference_revalidation", error.message)
                    })?;
            if blake3::hash(stable.content.as_bytes()).to_hex().as_str() != full_hash {
                return Err(RuntimeFailure::new(
                    "reference_changed_during_read",
                    "reference_revalidation",
                    "reference changed while its authoritative snapshot was being prepared",
                ));
            }
            let summary = ChatResolvedReference {
                reference_id: reference.reference_id.clone(),
                kind: target.kind().to_string(),
                path: Some(file.relative_path.clone()),
                range: target.range().copied(),
                inclusion_reason: bounded_text(
                    &reference.inclusion_reason,
                    MAX_REFERENCE_REASON_CHARS,
                ),
                provenance: "user_reference".to_string(),
                bytes: content.len(),
                content_hash: Some(injected_hash.clone()),
                origin: "user_attachment".to_string(),
                resolution: "resolved".to_string(),
                security_decision: "allow".to_string(),
                injection: "accepted".to_string(),
                bytes_injected: 0,
                reason: "explicit_user_reference".to_string(),
                full_content_hash: Some(full_hash.clone()),
            };
            Ok(ReferenceMaterial {
                summary,
                snapshot: Some(ReferenceSnapshot {
                    reference_id: reference.reference_id.clone(),
                    path: file.relative_path,
                    origin: "user_attachment".to_string(),
                    file_hash: full_hash,
                    injected_hash,
                    file_size: file.content.len(),
                    encoding: "utf-8".to_string(),
                    line_ending: detect_line_ending(&file.content).to_string(),
                    range,
                    content: content.clone(),
                    reason: "explicit_user_reference".to_string(),
                    git_state: reference_git_state(workspace, path),
                    workspace_id: workspace_id.to_string(),
                }),
                prompt_content: content,
            })
        }
    }
}

fn metadata_reference(reference: &ChatReference, prompt_content: String) -> ReferenceMaterial {
    ReferenceMaterial {
        summary: ChatResolvedReference {
            reference_id: reference.reference_id.clone(),
            kind: reference.target.kind().to_string(),
            path: None,
            range: None,
            inclusion_reason: bounded_text(&reference.inclusion_reason, MAX_REFERENCE_REASON_CHARS),
            provenance: "user_reference".to_string(),
            bytes: 0,
            content_hash: None,
            origin: "user_attachment".to_string(),
            resolution: "resolved".to_string(),
            security_decision: "allow".to_string(),
            injection: "identity_only".to_string(),
            bytes_injected: 0,
            reason: "metadata_identity_only".to_string(),
            full_content_hash: None,
        },
        prompt_content,
        snapshot: None,
    }
}

fn rejected_reference(
    reference: &ChatReference,
    rule_id: &str,
    reason: &str,
) -> ChatRejectedReference {
    ChatRejectedReference {
        reference_id: reference.reference_id.clone(),
        kind: reference.target.kind().to_string(),
        rule_id: bounded_text(rule_id, 128),
        reason: bounded_text(reason, 2 * 1024),
        path: reference.target.path().map(ToString::to_string),
        origin: "user_attachment".to_string(),
        injection: "refused".to_string(),
        reason_code: bounded_text(rule_id, 128),
    }
}

fn render_reference_material(material: &[ReferenceMaterial]) -> String {
    if material.is_empty() {
        return "none".to_string();
    }
    material
        .iter()
        .map(|item| {
            format!(
                "reference_id: {}\nkind: {}\npath: {}\nreason: {}\ncontent:\n{}",
                item.summary.reference_id,
                item.summary.kind,
                item.summary.path.as_deref().unwrap_or("metadata-only"),
                item.summary.inclusion_reason,
                item.prompt_content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn bounded_history(
    request: &ChatRequest,
    effective_scope: ChatContextScope,
) -> (String, Vec<String>, usize) {
    if request.history.is_empty() {
        return ("none".to_string(), Vec::new(), 0);
    }
    let mut selected = Vec::new();
    let mut chars = 0usize;
    let mut tokens = 0usize;
    let mut warnings = Vec::new();
    for turn in request.history.iter().rev() {
        let strict_history_metadata_missing = effective_scope != ChatContextScope::Automatic
            && (turn.workspace_id.as_deref() != Some(request.workspace_id.as_str())
                || turn.grounding_status.as_deref() != Some("grounded")
                || turn
                    .context_fingerprint
                    .as_deref()
                    .is_none_or(str::is_empty));
        if strict_history_metadata_missing
            || turn
                .workspace_id
                .as_deref()
                .is_some_and(|workspace| workspace != request.workspace_id)
            || turn
                .grounding_status
                .as_deref()
                .is_some_and(|status| status != "grounded")
            || turn.source_scope.is_some_and(|scope| {
                effective_scope == ChatContextScope::ReferencesPreferred
                    && scope == ChatContextScope::Automatic
            })
        {
            warnings.push("an incompatible or ungrounded historical turn was omitted".to_string());
            continue;
        }
        if selected.len() >= request.budgets.max_history_turns {
            warnings.push("older chat turns were omitted by the turn budget".to_string());
            break;
        }
        let mut content = bounded_text(&turn.content, MAX_HISTORY_TURN_CHARS);
        if inspect_sensitive_text(&content).is_some() {
            content = "[historical content omitted by secret scanning]".to_string();
            warnings.push("sensitive historical content was not reinjected".to_string());
        } else if looks_like_large_diff_or_report(&content) {
            content = format!(
                "[large historical diff/report omitted; result_id={}]",
                turn.result_id.as_deref().unwrap_or("unknown")
            );
            warnings.push("a large historical diff/report was summarized".to_string());
        }
        let turn_chars = content.chars().count();
        let turn_tokens = estimate_tokens(&content);
        if chars.saturating_add(turn_chars) > request.budgets.max_history_chars
            || tokens.saturating_add(turn_tokens) > request.budgets.max_history_tokens
        {
            warnings
                .push("older chat turns were omitted by the character/token budget".to_string());
            break;
        }
        chars += turn_chars;
        tokens += turn_tokens;
        selected.push(format!(
            "role: {:?}\ncommand: {}\ncontent:\n{}",
            turn.role,
            turn.command.map_or("none", ChatCommand::as_str),
            content
        ));
    }
    selected.reverse();
    let retained = selected.len();
    (selected.join("\n\n"), warnings, retained)
}

fn looks_like_large_diff_or_report(content: &str) -> bool {
    content.len() > 4 * 1024
        && (content.contains("diff --git")
            || content.contains("@@ -")
            || content.contains("\"context_files\"")
            || content.contains("# Full Report"))
}

fn project_summary(workspace: &Path) -> (String, String) {
    let workspace_report = inspect_workspace(workspace).ok();
    let git = capture_git_state(workspace).ok();
    let mut hasher = blake3::Hasher::new();
    hasher.update(workspace.to_string_lossy().as_bytes());
    if let Some(snapshot) = &git {
        for change in &snapshot.changes {
            hasher.update(change.path.as_bytes());
            hasher.update(&[change.index_status as u8, change.worktree_status as u8]);
            if let Some(fingerprint) = &change.content_fingerprint {
                hasher.update(fingerprint.as_bytes());
            }
        }
    }
    let digest = format!("state-{}", &hasher.finalize().to_hex()[..24]);
    let label = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    let summary = workspace_report.map_or_else(
        || {
            format!(
                "workspace: {label}\nrepository_state: {digest}\nproject inventory: unavailable"
            )
        },
        |report| {
            format!(
                concat!(
                    "workspace: {}\n",
                    "repository_state: {}\n",
                    "files_seen: {}\n",
                    "git_changes: {}\n",
                    "build: maven={}, gradle={}"
                ),
                label,
                digest,
                report.total_files_seen,
                git.as_ref().map_or(0, |snapshot| snapshot.changes.len()),
                report.has_maven,
                report.has_gradle
            )
        },
    );
    (summary, digest)
}

async fn run_grounded_assistant(
    app: Option<&OpticCode>,
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    emitter
        .send(ChatProtocolEventPayload::ContextStarted {
            requested_mode: request.context_mode,
        })
        .await
        .map_err(event_failure)?;
    let context_files = prepared
        .manifest
        .entries
        .iter()
        .map(|entry| ChatContextFile {
            path: entry.path.clone(),
            snippets: entry.ranges.len(),
            provenance: entry.origin.clone(),
        })
        .collect::<Vec<_>>();
    emitter
        .send(ChatProtocolEventPayload::ContextReady {
            requested_mode: request.context_mode,
            used_mode: None,
            analysis_complete: true,
            estimated_tokens: prepared.manifest.estimated_tokens,
            files: context_files.clone(),
        })
        .await
        .map_err(event_failure)?;
    emitter
        .send(ChatProtocolEventPayload::RetrievalProgress {
            query_count: 0,
            hit_count: 0,
        })
        .await
        .map_err(event_failure)?;
    if cancellation.is_cancelled() {
        return Err(RuntimeFailure::new(
            "request_cancelled",
            "grounded_context",
            "request was cancelled before grounded answer generation",
        ));
    }

    let force_document_inspection = request.command == ChatCommand::Inspect;
    let document_facts = if prepared.snapshots.len() == 1 {
        inspect_document_facts(
            &prepared.user_prompt,
            &prepared.snapshots[0],
            force_document_inspection,
        )
        .map_err(|error| {
            RuntimeFailure::new(
                "document_inspection_failed",
                "document_facts",
                format!("deterministic document inspection failed: {error:#}"),
            )
        })?
    } else {
        None
    };

    if let Some(facts) = document_facts {
        emitter
            .send(ChatProtocolEventPayload::DocumentInspectionCompleted {
                format: facts.format,
                facts: facts.response.claims.len(),
                model_calls: facts.model_calls,
            })
            .await
            .map_err(event_failure)?;
        let validation_started = Instant::now();
        let (evidence, compliance) = validate_grounded_answer(
            prepared,
            &facts.response,
            GroundingRoute::DocumentFacts,
            emitter,
        )
        .await?;
        let validation_ms = duration_ms_floor(validation_started.elapsed());
        revalidate_reference_snapshots(prepared)?;
        emit_text(emitter, &facts.response.answer).await?;
        return Ok(CommandOutcome {
            context_files,
            used_context_mode: None,
            metrics: grounded_metrics(
                request,
                prepared,
                GroundingRoute::DocumentFacts,
                &[],
                validation_ms,
                started,
            ),
            warnings: Vec::new(),
            route: GroundingRoute::DocumentFacts,
            grounding_response: Some(facts.response),
            evidence: Some(evidence),
            compliance: Some(compliance),
            rag_hits: 0,
        });
    }

    let app = app.ok_or_else(|| {
        RuntimeFailure::new(
            "provider_unavailable",
            "provider_setup",
            "the deterministic route did not match and the configured local LLM provider is unavailable",
        )
        .retriable(true)
    })?;
    if options.verify_model {
        app.ensure_model_available().await.map_err(|error| {
            RuntimeFailure::new(
                "model_unavailable",
                "provider_setup",
                format!("configured model verification failed: {error:#}"),
            )
            .retriable(true)
        })?;
    }
    emitter
        .send(ChatProtocolEventPayload::ProviderStarted {
            provider: request.provider,
            model: request.model.clone(),
            context_mode: request.context_mode,
        })
        .await
        .map_err(event_failure)?;
    let generations = generate_grounded_response(app, request, prepared, cancellation).await?;
    let response = parse_grounded_response(
        &generations
            .last()
            .expect("grounded generation always contains at least one result")
            .output,
    )?;
    if cancellation.is_cancelled() {
        return Err(RuntimeFailure::new(
            "request_cancelled",
            "grounded_generation",
            "request was cancelled before grounding validation",
        ));
    }
    let validation_started = Instant::now();
    let (evidence, compliance) =
        validate_grounded_answer(prepared, &response, GroundingRoute::ReferenceLlm, emitter)
            .await?;
    let validation_ms = duration_ms_floor(validation_started.elapsed());
    revalidate_reference_snapshots(prepared)?;
    emit_text(emitter, &response.answer).await?;
    let mut warnings = Vec::new();
    if generations.len() > 1 {
        warnings.push("one bounded JSON format correction was used".to_string());
    }
    Ok(CommandOutcome {
        context_files,
        used_context_mode: None,
        metrics: grounded_metrics(
            request,
            prepared,
            GroundingRoute::ReferenceLlm,
            &generations,
            validation_ms,
            started,
        ),
        warnings,
        route: GroundingRoute::ReferenceLlm,
        grounding_response: Some(response),
        evidence: Some(evidence),
        compliance: Some(compliance),
        rag_hits: 0,
    })
}

async fn generate_grounded_response(
    app: &OpticCode,
    request: &ChatRequest,
    prepared: &PreparedRequest,
    cancellation: &opticcode_llm::CancellationToken,
) -> std::result::Result<Vec<GenerationResult>, RuntimeFailure> {
    let schema = grounded_response_schema();
    let primary = app
        .generate_structured(
            format!("{}:grounded", request.request_id),
            prepared.prompt.clone(),
            schema.clone(),
            request.generation.max_output_tokens,
            Some(request.generation.temperature.unwrap_or(0.0)),
            request.generation.seed,
            cancellation.clone(),
        )
        .await
        .map_err(|error| {
            RuntimeFailure::new(
                "grounded_generation_failed",
                "grounded_generation",
                format!("structured local generation failed: {error:#}"),
            )
            .retriable(true)
        })?;
    if serde_json::from_str::<GroundedResponse>(&primary.output).is_ok() {
        return Ok(vec![primary]);
    }
    if cancellation.is_cancelled() {
        return Err(RuntimeFailure::new(
            "request_cancelled",
            "grounded_format_correction",
            "request was cancelled before the single format correction",
        ));
    }
    let correction_prompt = format!(
        concat!(
            "{}\n\n",
            "[FORMAT_CORRECTION]\n",
            "The previous output was not valid against the required JSON contract. ",
            "Perform only a format correction: preserve the same factual claims, use no new source, ",
            "and return exactly one JSON object.\n",
            "[PREVIOUS_OUTPUT]\n{}"
        ),
        prepared.prompt,
        bounded_text(&primary.output, 64 * 1024)
    );
    let corrected = app
        .generate_structured(
            format!("{}:grounded-format", request.request_id),
            correction_prompt,
            schema,
            request.generation.max_output_tokens,
            Some(0.0),
            request.generation.seed,
            cancellation.clone(),
        )
        .await
        .map_err(|error| {
            RuntimeFailure::new(
                "grounded_format_correction_failed",
                "grounded_format_correction",
                format!("the single format correction failed: {error:#}"),
            )
        })?;
    parse_grounded_response(&corrected.output)?;
    Ok(vec![primary, corrected])
}

fn parse_grounded_response(output: &str) -> std::result::Result<GroundedResponse, RuntimeFailure> {
    serde_json::from_str(output).map_err(|error| {
        RuntimeFailure::new(
            "grounded_response_invalid",
            "grounded_validation",
            format!(
                "structured grounded response is invalid after the allowed correction: {error}"
            ),
        )
    })
}

async fn validate_grounded_answer(
    prepared: &PreparedRequest,
    response: &GroundedResponse,
    route: GroundingRoute,
    emitter: &ChatEventEmitter,
) -> std::result::Result<(EvidenceValidationReport, ComplianceReport), RuntimeFailure> {
    emitter
        .send(ChatProtocolEventPayload::GroundingValidationStarted {
            route,
            evidence_mode: prepared.evidence_mode,
        })
        .await
        .map_err(event_failure)?;
    let evidence = validate_evidence(response, &prepared.manifest, prepared.evidence_mode);
    let compliance = validate_compliance(
        &prepared.user_prompt,
        response,
        &prepared.manifest,
        &prepared.snapshots,
    );
    emitter
        .send(ChatProtocolEventPayload::GroundingValidationCompleted {
            evidence: evidence.clone(),
            compliance: compliance.clone(),
        })
        .await
        .map_err(event_failure)?;
    if compliance.internal_context_leak {
        emitter
            .send(ChatProtocolEventPayload::InternalContextLeakDetected {
                markers: vec!["internal_context_marker".to_string()],
            })
            .await
            .map_err(event_failure)?;
    }
    if !evidence.valid || !compliance.compliant {
        let mut errors = evidence.errors.clone();
        errors.extend(compliance.errors.clone());
        errors.truncate(128);
        emitter
            .send(ChatProtocolEventPayload::TaskComplianceFailed {
                errors: errors.clone(),
            })
            .await
            .map_err(event_failure)?;
        return Err(RuntimeFailure::new(
            "task_compliance_failed",
            "grounding_validation",
            if compliance.internal_context_leak {
                "OpticCode refused the model response because it contained unauthorized internal context"
            } else if compliance.cross_file_leak {
                "OpticCode refused the model response because it mentioned unauthorized sources"
            } else {
                "OpticCode refused the response because its evidence or task compliance was invalid"
            },
        ));
    }
    Ok((evidence, compliance))
}

fn revalidate_reference_snapshots(
    prepared: &PreparedRequest,
) -> std::result::Result<(), RuntimeFailure> {
    for snapshot in &prepared.snapshots {
        let current = read_safe_workspace_file(
            &prepared.workspace,
            Path::new(&snapshot.path),
            MAX_SAFE_REFERENCE_FILE_BYTES,
        )
        .map_err(|error| {
            RuntimeFailure::new(
                "reference_revalidation_failed",
                "grounding_revalidation",
                format!("{}: {}", error.rule_id, error.message),
            )
        })?;
        let current_hash = blake3::hash(current.content.as_bytes())
            .to_hex()
            .to_string();
        if current_hash != snapshot.file_hash || current.content.len() != snapshot.file_size {
            return Err(RuntimeFailure::new(
                "reference_snapshot_stale",
                "grounding_revalidation",
                "an injected reference changed before the grounded response could be accepted",
            ));
        }
    }
    Ok(())
}

fn grounded_metrics(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    route: GroundingRoute,
    generations: &[GenerationResult],
    validation_ms: u64,
    started: Instant,
) -> ChatMetrics {
    let prompt_tokens = sum_optional_u64(
        generations
            .iter()
            .map(|generation| generation.usage.prompt_tokens),
    );
    let generated_tokens = sum_optional_u64(
        generations
            .iter()
            .map(|generation| generation.usage.generated_tokens),
    );
    let generation_ms = generations
        .iter()
        .filter_map(|generation| generation.timings.generation_ms)
        .sum::<u64>();
    let generated_tokens_per_second = generated_tokens
        .filter(|_| generation_ms > 0)
        .map(|tokens| tokens as f64 / (generation_ms as f64 / 1_000.0));
    let provider_client_ms = generations
        .iter()
        .map(|generation| generation.timings.client_ms)
        .sum::<u64>();
    let provider_total_ms = generations
        .iter()
        .filter_map(|generation| generation.timings.provider_total_ms)
        .sum::<u64>();
    let provider_load_ms = generations
        .iter()
        .filter_map(|generation| generation.timings.load_ms)
        .sum::<u64>();
    let prompt_eval_ms = generations
        .iter()
        .filter_map(|generation| generation.timings.prompt_eval_ms)
        .sum::<u64>();
    let mut phases = vec![
        timing_phase(
            "reference_resolution",
            prepared.reference_resolution_ms,
            "opticcode-core",
            &["references_started", "references_completed"],
        ),
        timing_phase(
            "prompt_build",
            prepared.prompt_build_ms,
            "opticcode-core",
            &["context_started", "context_completed"],
        ),
    ];
    if !generations.is_empty() {
        phases.push(timing_phase(
            "provider_client",
            provider_client_ms,
            "opticcode-llm",
            &["provider_started", "provider_completed"],
        ));
        phases.push(timing_phase(
            "provider_total",
            provider_total_ms,
            "ollama",
            &["provider_reported_total"],
        ));
        phases.push(timing_phase(
            "provider_generation",
            generation_ms,
            "ollama",
            &["provider_token_generation"],
        ));
        phases.push(timing_phase(
            "provider_load",
            provider_load_ms,
            "ollama",
            &["provider_model_load"],
        ));
        phases.push(timing_phase(
            "provider_prompt_eval",
            prompt_eval_ms,
            "ollama",
            &["provider_prompt_evaluation"],
        ));
    }
    phases.push(timing_phase(
        "grounding_validation",
        validation_ms,
        "opticcode-core",
        &["validation_started", "validation_completed"],
    ));
    ChatMetrics {
        preparation_ms: prepared
            .reference_resolution_ms
            .saturating_add(prepared.prompt_build_ms),
        total_ms: elapsed_ms(started),
        estimated_prompt_tokens: estimate_tokens(&prepared.prompt),
        prompt_tokens,
        generated_tokens,
        generated_tokens_per_second,
        timing: Some(timing_report(request, phases)),
        route: route.as_str().to_string(),
    }
}

fn sum_optional_u64(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    let mut seen = false;
    let total = values.fold(0u64, |total, value| {
        if let Some(value) = value {
            seen = true;
            total.saturating_add(value)
        } else {
            total
        }
    });
    seen.then_some(total)
}

async fn run_assistant(
    app: &OpticCode,
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    cancellation: &opticcode_llm::CancellationToken,
    options: &ChatRuntimeOptions,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    emitter
        .send(ChatProtocolEventPayload::ContextStarted {
            requested_mode: request.context_mode,
        })
        .await
        .map_err(event_failure)?;
    let (events, mut receiver) = assistant_event_channel(DEFAULT_ASSISTANT_EVENT_CAPACITY)
        .map_err(|error| {
            RuntimeFailure::new("event_channel", "assistant_setup", error.to_string())
        })?;
    let assistant_session = AssistantProtocolSession {
        request_id: request.request_id.clone(),
        events,
        cancellation: cancellation.clone(),
    };
    let execution: Pin<Box<dyn Future<Output = Result<AssistantCommandReport>> + '_>> =
        match request.command {
            ChatCommand::Ask => Box::pin(app.ask_with_protocol(
                AskOptions {
                    workspace: prepared.workspace.clone(),
                    prompt: prepared.prompt.clone(),
                    profile: Some(request.profile.clone()),
                    include_memory: true,
                    include_rag: request.budgets.rag_hits > 0,
                    rag_index: options.rag_index.clone(),
                    rag_limit: request.budgets.rag_hits,
                    brief: request.generation.brief,
                    max_tokens: Some(request.generation.max_output_tokens),
                    temperature: request.generation.temperature,
                    seed: request.generation.seed,
                    context_mode: request.context_mode,
                    fallback_policy: ContextFallbackPolicy::Legacy,
                    compare_generate: request.generation.compare_generate,
                    verify_model: options.verify_model,
                },
                assistant_session,
            )),
            ChatCommand::Plan => Box::pin(app.plan_with_protocol(
                PlanOptions {
                    workspace: prepared.workspace.clone(),
                    goal: prepared.prompt.clone(),
                    profile: Some(request.profile.clone()),
                    include_memory: true,
                    include_rag: request.budgets.rag_hits > 0,
                    rag_index: options.rag_index.clone(),
                    rag_limit: request.budgets.rag_hits,
                    brief: request.generation.brief,
                    max_tokens: Some(request.generation.max_output_tokens),
                    temperature: request.generation.temperature,
                    seed: request.generation.seed,
                    context_mode: request.context_mode,
                    fallback_policy: ContextFallbackPolicy::Legacy,
                    compare_generate: request.generation.compare_generate,
                    verify_model: options.verify_model,
                },
                assistant_session,
            )),
            _ => unreachable!("run_assistant accepts only ask or plan"),
        };
    tokio::pin!(execution);
    let mut result = None;
    let mut events_open = true;
    let mut next_sequence = 0u64;
    let mut terminal_count = 0usize;
    while result.is_none() || events_open {
        tokio::select! {
            event = receiver.recv(), if events_open => {
                match event {
                    Some(event) => {
                        if event.sequence != next_sequence {
                            return Err(RuntimeFailure::new(
                                "assistant_sequence_mismatch",
                                "assistant_stream",
                                format!("expected assistant sequence {next_sequence}, received {}", event.sequence),
                            ));
                        }
                        next_sequence = next_sequence.saturating_add(1);
                        if event.is_terminal() {
                            terminal_count += 1;
                        } else if terminal_count > 0 {
                            return Err(RuntimeFailure::new(
                                "assistant_event_after_terminal",
                                "assistant_stream",
                                "assistant emitted an event after its terminal event",
                            ));
                        }
                        translate_assistant_event(&event.payload, request, emitter).await?;
                    }
                    None => events_open = false,
                }
            }
            completed = &mut execution, if result.is_none() => {
                result = Some(completed);
            }
        }
    }
    if terminal_count != 1 {
        return Err(RuntimeFailure::new(
            "assistant_terminal_mismatch",
            "assistant_stream",
            format!("assistant emitted {terminal_count} terminal events"),
        ));
    }
    let report = result
        .ok_or_else(|| {
            RuntimeFailure::new(
                "assistant_result_missing",
                "assistant_stream",
                "assistant execution ended without a report",
            )
        })?
        .map_err(|error| {
            RuntimeFailure::new(
                "assistant_failed",
                "assistant_runtime",
                format!("{error:#}"),
            )
            .retriable(true)
        })?;
    if !report.success {
        let cancelled = report
            .errors
            .iter()
            .any(|error| error.code.contains("cancel"));
        return Err(RuntimeFailure::new(
            if cancelled {
                "request_cancelled"
            } else {
                "assistant_failed"
            },
            "assistant_runtime",
            report
                .errors
                .iter()
                .map(|error| format!("{}: {}", error.code, error.message))
                .collect::<Vec<_>>()
                .join("; "),
        )
        .retriable(!cancelled));
    }
    if !report.runs.iter().any(|run| run.generated) {
        let comparison = report.context.comparison.as_ref().map_or_else(
            || "Context comparison completed without model generation.".to_string(),
            |comparison| {
                format!(
                    concat!(
                        "Context comparison completed without model generation.\n\n",
                        "- legacy: {} estimated tokens, {} files\n",
                        "- symbol: {} estimated tokens, {} files\n",
                        "- estimated delta: {} tokens"
                    ),
                    comparison.legacy_estimated_tokens,
                    comparison.legacy_files,
                    comparison.symbol_estimated_tokens,
                    comparison.symbol_files,
                    comparison.estimated_token_delta
                )
            },
        );
        emit_text(emitter, &comparison).await?;
    }
    let context_files = report
        .context
        .variants
        .iter()
        .flat_map(|variant| {
            variant.report.files.iter().map(|file| ChatContextFile {
                path: file.path.clone(),
                snippets: file.snippets,
                provenance: "context_discovery".to_string(),
            })
        })
        .collect::<Vec<_>>();
    let generated = report.generated_run();
    let preparation_ms = report.preparation_duration_us.saturating_add(999) / 1_000;
    let mut phases = vec![timing_phase(
        "context_build",
        preparation_ms,
        "opticcode-core",
        &["context_started", "context_completed"],
    )];
    if let Some(generation) = generated.and_then(|run| run.metrics.as_ref()) {
        phases.push(timing_phase(
            "provider_client",
            generation.client_ms,
            "opticcode-llm",
            &["provider_started", "provider_completed"],
        ));
        if let Some(provider_total_ms) = generation.ollama_total_ms {
            phases.push(timing_phase(
                "provider_total",
                provider_total_ms,
                "ollama",
                &["provider_reported_total"],
            ));
        }
        if let Some(generation_ms) = generation.generation_ms {
            phases.push(timing_phase(
                "provider_generation",
                generation_ms,
                "ollama",
                &["provider_token_generation"],
            ));
        }
    }
    let metrics = ChatMetrics {
        preparation_ms,
        total_ms: elapsed_ms(started),
        estimated_prompt_tokens: generated
            .map(|run| run.prompt.estimated_tokens)
            .unwrap_or_else(|| {
                report
                    .runs
                    .first()
                    .map_or(0, |run| run.prompt.estimated_tokens)
            }),
        prompt_tokens: generated.and_then(|run| {
            run.metrics
                .as_ref()
                .and_then(|metrics| metrics.prompt_eval_count)
        }),
        generated_tokens: generated.and_then(|run| {
            run.metrics
                .as_ref()
                .and_then(|metrics| metrics.generated_tokens)
        }),
        generated_tokens_per_second: generated.and_then(|run| {
            run.metrics
                .as_ref()
                .and_then(|metrics| metrics.generated_tokens_per_second)
        }),
        timing: Some(timing_report(request, phases)),
        route: GroundingRoute::AutomaticAssistant.as_str().to_string(),
    };
    let rag_hits = report.rag.hits.len();
    Ok(CommandOutcome {
        context_files,
        used_context_mode: report.used_context_mode,
        metrics,
        warnings: report.warnings,
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits,
    })
}

async fn translate_assistant_event(
    payload: &AssistantProtocolEventPayload,
    request: &ChatRequest,
    emitter: &ChatEventEmitter,
) -> std::result::Result<(), RuntimeFailure> {
    match payload {
        AssistantProtocolEventPayload::ContextPrepared {
            requested_context_mode,
            used_context_mode,
            analysis_complete,
            ..
        } => {
            emitter
                .send(ChatProtocolEventPayload::ContextReady {
                    requested_mode: *requested_context_mode,
                    used_mode: *used_context_mode,
                    analysis_complete: *analysis_complete,
                    estimated_tokens: 0,
                    files: Vec::new(),
                })
                .await
                .map_err(event_failure)?;
            emitter
                .send(ChatProtocolEventPayload::RetrievalProgress {
                    query_count: 0,
                    hit_count: 0,
                })
                .await
                .map_err(event_failure)?;
        }
        AssistantProtocolEventPayload::ProviderEvent {
            context_mode,
            event,
        } if event.is_started() => {
            emitter
                .send(ChatProtocolEventPayload::ProviderStarted {
                    provider: request.provider,
                    model: request.model.clone(),
                    context_mode: *context_mode,
                })
                .await
                .map_err(event_failure)?;
        }
        AssistantProtocolEventPayload::ProviderEvent { event, .. } => {
            if let Some(delta) = event.output_delta() {
                emit_text(emitter, delta).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn run_context_command(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    emitter
        .send(ChatProtocolEventPayload::ContextStarted {
            requested_mode: request.context_mode,
        })
        .await
        .map_err(event_failure)?;
    let workspace = prepared.workspace.clone();
    let task = request.prompt.clone();
    let mode = request.context_mode;
    let context = tokio::task::spawn_blocking(move || {
        prepare_assistant_context(&workspace, &task, mode, ContextFallbackPolicy::Legacy)
    })
    .await
    .map_err(|error| RuntimeFailure::new("context_join", "context", error.to_string()))?
    .map_err(|error| RuntimeFailure::new("context_failed", "context", format!("{error:#}")))?;
    let files = context
        .variants
        .iter()
        .flat_map(|variant| {
            variant.report.files.iter().map(|file| ChatContextFile {
                path: file.path.clone(),
                snippets: file.snippets,
                provenance: "context_discovery".to_string(),
            })
        })
        .collect::<Vec<_>>();
    let estimated_tokens = context
        .variants
        .iter()
        .map(|variant| variant.report.estimated_tokens)
        .sum();
    emitter
        .send(ChatProtocolEventPayload::ContextReady {
            requested_mode: request.context_mode,
            used_mode: context.used_mode,
            analysis_complete: context.analysis_complete,
            estimated_tokens,
            files: files.clone(),
        })
        .await
        .map_err(event_failure)?;
    let mut markdown = format!(
        "Context `{}` prepared: {} file(s), about {} tokens.",
        context.used_mode.unwrap_or(request.context_mode),
        files.len(),
        estimated_tokens
    );
    for file in files.iter().take(20) {
        markdown.push_str(&format!(
            "\n- `{}` ({} snippet(s))",
            file.path, file.snippets
        ));
    }
    emit_text(emitter, &markdown).await?;
    Ok(CommandOutcome {
        context_files: files,
        used_context_mode: context.used_mode,
        metrics: command_metrics(started, estimated_tokens),
        warnings: context
            .fallback
            .into_iter()
            .map(|fallback| fallback.warning)
            .collect(),
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    })
}

async fn run_analyze_command(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let workspace = prepared.workspace.clone();
    let report = tokio::task::spawn_blocking(move || {
        analyze_java_syntax(&workspace, JavaSyntaxOptions::default())
    })
    .await
    .map_err(|error| RuntimeFailure::new("analysis_join", "java_syntax", error.to_string()))?
    .map_err(|error| RuntimeFailure::new("analysis_failed", "java_syntax", format!("{error:#}")))?;
    for (index, (file, diagnostic)) in report
        .files
        .iter()
        .flat_map(|file| {
            file.diagnostics
                .iter()
                .map(move |diagnostic| (file, diagnostic))
        })
        .take(32)
        .enumerate()
    {
        emitter
            .send(ChatProtocolEventPayload::Finding {
                finding_id: format!("{}-diagnostic-{index}", request.request_id),
                severity: "error".to_string(),
                message: diagnostic.message.clone(),
                path: normalized_path(&file.path),
                range: Some(ChatTextRange {
                    start: ChatTextPosition {
                        line: bounded_u32(diagnostic.range.start.row),
                        character: bounded_u32(diagnostic.range.start.column),
                    },
                    end: ChatTextPosition {
                        line: bounded_u32(diagnostic.range.end.row),
                        character: bounded_u32(diagnostic.range.end.column),
                    },
                }),
            })
            .await
            .map_err(event_failure)?;
    }
    let markdown = format!(
        concat!(
            "Java analysis completed in read-only mode.\n\n",
            "- files parsed: {}\n",
            "- syntax error files: {}\n",
            "- symbols: {}\n",
            "- references: {}\n",
            "- complete: {}"
        ),
        report.parsed_files,
        report.syntax_error_files,
        report.counts.symbols,
        report.counts.references,
        report.analysis_complete
    );
    emit_text(emitter, &markdown).await?;
    Ok(CommandOutcome {
        context_files: report
            .files
            .iter()
            .take(64)
            .map(|file| ChatContextFile {
                path: normalized_path(&file.path),
                snippets: file.symbols.len(),
                provenance: "java_analysis".to_string(),
            })
            .collect(),
        used_context_mode: None,
        metrics: command_metrics(started, 0),
        warnings: report.warnings,
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    })
}

async fn run_index_command(
    _request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let workspace = prepared.workspace.clone();
    let report = tokio::task::spawn_blocking(move || {
        analyze_java_index(&workspace, JavaIndexOptions::default())
    })
    .await
    .map_err(|error| RuntimeFailure::new("index_join", "java_index", error.to_string()))?
    .map_err(|error| RuntimeFailure::new("index_failed", "java_index", format!("{error:#}")))?;
    let markdown = format!(
        concat!(
            "Java symbol index built in memory (read-only).\n\n",
            "- files: {}/{} parsed\n",
            "- declarations: {}\n",
            "- references: {}\n",
            "- exact: {}\n",
            "- ambiguous: {}\n",
            "- unresolved: {}"
        ),
        report.source.parsed_files,
        report.source.discovered_files,
        report.counts.declarations,
        report.counts.references,
        report.counts.exact,
        report.counts.ambiguous,
        report.counts.unresolved
    );
    emit_text(emitter, &markdown).await?;
    Ok(CommandOutcome {
        context_files: report
            .files
            .iter()
            .take(64)
            .map(|file| ChatContextFile {
                path: normalized_path(&file.path),
                snippets: report
                    .symbols
                    .iter()
                    .filter(|symbol| symbol.file == file.path)
                    .count(),
                provenance: "java_index".to_string(),
            })
            .collect(),
        used_context_mode: None,
        metrics: command_metrics(started, 0),
        warnings: report.warnings,
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    })
}

async fn run_legacy_command(
    _request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let workspace = prepared.workspace.clone();
    let report = tokio::task::spawn_blocking(move || {
        propose_java_edits(&workspace, JavaEditOptions::default())
    })
    .await
    .map_err(|error| RuntimeFailure::new("legacy_join", "java_edits", error.to_string()))?
    .map_err(|error| RuntimeFailure::new("legacy_failed", "java_edits", format!("{error:#}")))?;
    let markdown = format!(
        concat!(
            "Legacy scan completed without writing files.\n\n",
            "- references examined: {}\n",
            "- legacy candidates: {}\n",
            "- verified proposals: {} in {} file(s)\n",
            "- safe downstream: {}"
        ),
        report.counts.references_examined,
        report.counts.legacy_candidates,
        report.counts.proposals,
        report.counts.files_with_proposals,
        report.safe_to_apply
    );
    emit_text(emitter, &markdown).await?;
    Ok(CommandOutcome {
        context_files: report
            .file_validations
            .iter()
            .map(|file| ChatContextFile {
                path: normalized_path(&file.path),
                snippets: file.edit_count,
                provenance: "legacy_analysis".to_string(),
            })
            .collect(),
        used_context_mode: None,
        metrics: command_metrics(started, 0),
        warnings: report.warnings,
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    })
}

async fn run_status_command(
    request: &ChatRequest,
    prepared: &PreparedRequest,
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let git = capture_git_state(&prepared.workspace).ok();
    let markdown = format!(
        concat!(
            "OpticCode chat status\n\n",
            "- security mode: `read_only`\n",
            "- provider: `{}`\n",
            "- model: `{}`\n",
            "- context: `{}`\n",
            "- repository state: `{}`\n",
            "- Git changes: {}\n",
            "- policy: deny-by-default POLICY-001 active\n",
            "- chat edits: verified worktree proposals with native approval for original apply"
        ),
        request.provider,
        request.model,
        request.context_mode,
        prepared.repository_state,
        git.as_ref().map_or(0, |snapshot| snapshot.changes.len())
    );
    emit_text(emitter, &markdown).await?;
    Ok(CommandOutcome {
        context_files: Vec::new(),
        used_context_mode: None,
        metrics: command_metrics(started, 0),
        warnings: Vec::new(),
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    })
}

async fn run_runs_command(
    request: &ChatRequest,
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let markdown = if request.client.recent_run_ids.is_empty() {
        "No bounded run IDs are recorded for this workspace session. Open the Runs view for the dashboard."
            .to_string()
    } else {
        format!(
            "Recent run IDs for this workspace session:\n{}",
            request
                .client
                .recent_run_ids
                .iter()
                .take(20)
                .map(|id| format!("- `{}`", bounded_text(id, MAX_ID_BYTES)))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    emit_text(emitter, &markdown).await?;
    Ok(empty_outcome(started))
}

async fn run_help_command(
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    let markdown = concat!(
        "OpticCode Local is protected by the deny-by-default POLICY-001 runtime.\n\n",
        "- `/ask`: answer with bounded project, RAG, history, and explicit references\n",
        "- `/plan`: produce a plan without writing\n",
        "- `/inspect`: inspect deterministic facts in an attached structured document\n",
        "- `/context`: inspect legacy/symbol context\n",
        "- `/analyze`, `/index`, `/legacy`: run read-only Java analysis\n",
        "- `/status`, `/runs`: inspect local state\n",
        "- `/fix`: generate and verify a bounded edit proposal in a disposable worktree\n",
        "- `/verify`, `/diff`: revalidate or review a stored proposal\n",
        "- `/apply`, `/rollback`: require a native VS Code modal and one-shot Policy approval\n\n",
        "Attached files are never implicit write permission. Sensitive files, paths outside the workspace, and symlinks/junctions are refused."
    );
    emit_text(emitter, markdown).await?;
    Ok(empty_outcome(started))
}

pub(crate) async fn emit_text(
    emitter: &ChatEventEmitter,
    text: &str,
) -> std::result::Result<(), RuntimeFailure> {
    if text.is_empty() {
        return Ok(());
    }
    let mut remaining = text;
    while !remaining.is_empty() {
        let mut end = remaining
            .len()
            .min(OUTPUT_CHUNK_BYTES.min(MAX_CHAT_EVENT_TEXT_BYTES));
        while end > 0 && !remaining.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            return Err(RuntimeFailure::new(
                "output_encoding",
                "event_rendering",
                "could not split UTF-8 output at a valid boundary",
            ));
        }
        emitter
            .send(ChatProtocolEventPayload::TokenDelta {
                text: remaining[..end].to_string(),
            })
            .await
            .map_err(event_failure)?;
        remaining = &remaining[end..];
    }
    Ok(())
}

fn extract_utf16_range_snapshot(
    content: &str,
    range: ChatTextRange,
) -> std::result::Result<(String, ContextManifestRange), RuntimeFailure> {
    let start = position_to_byte(content, range.start)?;
    let end = position_to_byte(content, range.end)?;
    if start > end {
        return Err(RuntimeFailure::new(
            "reference.invalid_range",
            "reference_resolution",
            "range start is after range end",
        ));
    }
    Ok((
        content[start..end].to_string(),
        manifest_range_from_bytes(content, start, end),
    ))
}

fn manifest_range_from_bytes(
    content: &str,
    start_byte: usize,
    end_byte: usize,
) -> ContextManifestRange {
    let start_line = 1 + content[..start_byte.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u32;
    let end_anchor = end_byte.saturating_sub(1).min(content.len());
    let end_line = if end_byte == 0 {
        start_line
    } else {
        1 + content[..end_anchor]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count() as u32
    };
    ContextManifestRange {
        start_line,
        end_line: end_line.max(start_line),
        start_byte,
        end_byte,
    }
}

fn detect_line_ending(content: &str) -> &'static str {
    let crlf = content.matches("\r\n").count();
    let lf = content.matches('\n').count();
    match (crlf, lf.saturating_sub(crlf)) {
        (0, 0) => "none",
        (0, _) => "lf",
        (_, 0) => "crlf",
        _ => "mixed",
    }
}

fn reference_git_state(workspace: &Path, path: &str) -> String {
    let normalized = path.replace('\\', "/");
    capture_git_state(workspace)
        .ok()
        .and_then(|state| {
            state
                .changes
                .into_iter()
                .find(|change| change.path.replace('\\', "/") == normalized)
                .map(|change| format!("changed:{}{}", change.index_status, change.worktree_status))
        })
        .unwrap_or_else(|| "clean_or_untracked".to_string())
}

fn position_to_byte(
    content: &str,
    position: ChatTextPosition,
) -> std::result::Result<usize, RuntimeFailure> {
    let line_index = usize::try_from(position.line).map_err(|_| {
        RuntimeFailure::new(
            "reference.invalid_range",
            "reference_resolution",
            "line index cannot be represented",
        )
    })?;
    let mut line_start = 0usize;
    for _ in 0..line_index {
        let relative = content[line_start..].find('\n').ok_or_else(|| {
            RuntimeFailure::new(
                "reference.invalid_range",
                "reference_resolution",
                "range line is outside the referenced file",
            )
        })?;
        line_start = line_start.saturating_add(relative).saturating_add(1);
    }
    let line_end = content[line_start..]
        .find('\n')
        .map(|offset| line_start + offset)
        .unwrap_or(content.len());
    let logical_end = if line_end > line_start && content.as_bytes()[line_end - 1] == b'\r' {
        line_end - 1
    } else {
        line_end
    };
    let line = &content[line_start..logical_end];
    let target = usize::try_from(position.character).map_err(|_| {
        RuntimeFailure::new(
            "reference.invalid_range",
            "reference_resolution",
            "UTF-16 column cannot be represented",
        )
    })?;
    let mut utf16 = 0usize;
    for (byte, character) in line.char_indices() {
        if utf16 == target {
            return Ok(line_start + byte);
        }
        utf16 = utf16.saturating_add(character.len_utf16());
        if utf16 > target {
            return Err(RuntimeFailure::new(
                "reference.invalid_range",
                "reference_resolution",
                "UTF-16 column splits a surrogate pair",
            ));
        }
    }
    if utf16 == target {
        Ok(logical_end)
    } else {
        Err(RuntimeFailure::new(
            "reference.invalid_range",
            "reference_resolution",
            "range column is outside the referenced line",
        ))
    }
}

fn extract_symbol_context(
    content: &str,
    symbol: &str,
) -> std::result::Result<String, RuntimeFailure> {
    let offset = content.find(symbol).ok_or_else(|| {
        RuntimeFailure::new(
            "reference.symbol_not_found",
            "reference_resolution",
            "symbol was not found in its referenced file snapshot",
        )
    })?;
    let line_start = content[..offset].rfind('\n').map_or(0, |value| value + 1);
    let line_end = content[offset..]
        .find('\n')
        .map_or(content.len(), |value| offset + value);
    Ok(content[line_start..line_end].to_string())
}

fn empty_outcome(started: Instant) -> CommandOutcome {
    CommandOutcome {
        context_files: Vec::new(),
        used_context_mode: None,
        metrics: command_metrics(started, 0),
        warnings: Vec::new(),
        route: GroundingRoute::AutomaticAssistant,
        grounding_response: None,
        evidence: None,
        compliance: None,
        rag_hits: 0,
    }
}

fn command_metrics(started: Instant, estimated_prompt_tokens: usize) -> ChatMetrics {
    ChatMetrics {
        preparation_ms: elapsed_ms(started),
        total_ms: elapsed_ms(started),
        estimated_prompt_tokens,
        prompt_tokens: None,
        generated_tokens: None,
        generated_tokens_per_second: None,
        timing: None,
        route: GroundingRoute::AutomaticAssistant.as_str().to_string(),
    }
}

fn timing_report(request: &ChatRequest, phases: Vec<ChatTimingPhase>) -> ChatTimingReport {
    ChatTimingReport {
        schema_version: 1,
        request_id: request.request_id.clone(),
        run_id: request.request_id.clone(),
        workspace_id: request.workspace_id.clone(),
        command: request.command.as_str().to_string(),
        unit: "milliseconds".to_string(),
        clock: "std::time::Instant".to_string(),
        phases,
    }
}

fn timing_phase(
    name: &str,
    duration_ms: u64,
    measured_by: &str,
    includes: &[&str],
) -> ChatTimingPhase {
    ChatTimingPhase {
        name: name.to_string(),
        duration_ms,
        measured_by: measured_by.to_string(),
        includes: includes.iter().map(|value| (*value).to_string()).collect(),
    }
}

fn duration_ms_floor(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn event_failure(error: anyhow::Error) -> RuntimeFailure {
    RuntimeFailure::new("event_delivery", "chat_protocol", error.to_string()).retriable(true)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn estimate_tokens(value: &str) -> usize {
    value.chars().count().saturating_add(3) / 4
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...[truncated]");
    }
    output
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn bounded_u32(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use opticcode_llm::{
        EventSink, FinishReason, GenerationRequest, GenerationResult, HealthReport, HealthRequest,
        HealthStatus, LlmProtocolEvent, LlmProtocolEventPayload, LlmProvider, ModelInfo,
        ProviderCapabilities, ProviderError, ProviderId, LLM_PROTOCOL_SCHEMA_VERSION,
    };

    use super::*;
    use crate::chat_protocol::{
        chat_event_channel, ChatBudgets, ChatClientMetadata, ChatExpectedProtocols,
        ChatGenerationOptions, ChatHistoryRole, ChatHistoryTurn, ChatReferenceTarget,
        DEFAULT_CHAT_EVENT_CAPACITY,
    };

    struct MockProvider;

    struct StructuredMockProvider {
        outputs: Mutex<VecDeque<String>>,
        calls: AtomicUsize,
    }

    impl StructuredMockProvider {
        fn new(outputs: impl IntoIterator<Item = String>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into_iter().collect()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    fn generation(request: &GenerationRequest) -> GenerationResult {
        generation_with_output(request, "mock answer")
    }

    fn generation_with_output(request: &GenerationRequest, output: &str) -> GenerationResult {
        GenerationResult {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            provider: ProviderId::Ollama,
            model: request.model.clone(),
            output: output.to_string(),
            finish_reason: FinishReason::Stop,
            prompt_chars: request.prompt.chars().count(),
            usage: opticcode_llm::GenerationUsage {
                prompt_tokens: Some(42),
                generated_tokens: Some(2),
            },
            timings: opticcode_llm::GenerationTimings {
                client_ms: 5,
                provider_total_ms: Some(4),
                load_ms: None,
                prompt_eval_ms: Some(2),
                generation_ms: Some(2),
            },
        }
    }

    #[async_trait]
    impl LlmProvider for StructuredMockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::Ollama
        }

        fn endpoint(&self) -> &str {
            "http://localhost:11434"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            MockProvider.capabilities()
        }

        async fn health(
            &self,
            request: HealthRequest,
        ) -> std::result::Result<HealthReport, ProviderError> {
            MockProvider.health(request).await
        }

        async fn list_models(&self) -> std::result::Result<Vec<ModelInfo>, ProviderError> {
            Ok(Vec::new())
        }

        async fn generate(
            &self,
            request: GenerationRequest,
            _cancellation: opticcode_llm::CancellationToken,
        ) -> std::result::Result<GenerationResult, ProviderError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let output = self
                .outputs
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "{}".to_string());
            Ok(generation_with_output(&request, &output))
        }

        async fn stream(
            &self,
            request: GenerationRequest,
            _events: EventSink,
            cancellation: opticcode_llm::CancellationToken,
        ) -> std::result::Result<GenerationResult, ProviderError> {
            self.generate(request, cancellation).await
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::Ollama
        }

        fn endpoint(&self) -> &str {
            "http://localhost:11434"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                local_only: true,
                health: true,
                model_listing: true,
                generation: true,
                streaming: true,
                cancellation: true,
                token_usage: true,
                provider_timings: true,
                deterministic_seed: true,
            }
        }

        async fn health(
            &self,
            request: HealthRequest,
        ) -> std::result::Result<HealthReport, ProviderError> {
            Ok(HealthReport {
                schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
                provider: ProviderId::Ollama,
                endpoint: self.endpoint().to_string(),
                status: HealthStatus::Healthy,
                reachable: true,
                latency_ms: 1,
                model_count: 1,
                requested_model: request.model,
                model_available: Some(true),
            })
        }

        async fn list_models(&self) -> std::result::Result<Vec<ModelInfo>, ProviderError> {
            Ok(Vec::new())
        }

        async fn generate(
            &self,
            request: GenerationRequest,
            _cancellation: opticcode_llm::CancellationToken,
        ) -> std::result::Result<GenerationResult, ProviderError> {
            Ok(generation(&request))
        }

        async fn stream(
            &self,
            request: GenerationRequest,
            events: EventSink,
            _cancellation: opticcode_llm::CancellationToken,
        ) -> std::result::Result<GenerationResult, ProviderError> {
            events
                .send(LlmProtocolEvent::new(
                    request.request_id.clone(),
                    0,
                    LlmProtocolEventPayload::Started {
                        provider: ProviderId::Ollama,
                        model: request.model.clone(),
                    },
                ))
                .await
                .unwrap();
            events
                .send(LlmProtocolEvent::new(
                    request.request_id.clone(),
                    1,
                    LlmProtocolEventPayload::Delta {
                        text: "mock answer".to_string(),
                    },
                ))
                .await
                .unwrap();
            let result = generation(&request);
            events
                .send(LlmProtocolEvent::new(
                    request.request_id.clone(),
                    2,
                    LlmProtocolEventPayload::Completed {
                        result: result.clone(),
                    },
                ))
                .await
                .unwrap();
            Ok(result)
        }
    }

    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src/main/java/test")).unwrap();
        fs::write(
            temp.path().join("src/main/java/test/Plugin.java"),
            "package test;\npublic class Plugin { String value = \"ete\"; }\n",
        )
        .unwrap();
        temp
    }

    fn request(root: &Path, command: ChatCommand) -> ChatRequest {
        ChatRequest {
            schema_version: CHAT_PROTOCOL_SCHEMA_VERSION,
            protocol: CHAT_PROTOCOL_ID.to_string(),
            request_id: format!("chat-test-{}", command.as_str()),
            workspace_id: "workspace-test".to_string(),
            workspace_root: root.to_string_lossy().to_string(),
            command,
            prompt: if matches!(command, ChatCommand::Status | ChatCommand::Help) {
                String::new()
            } else {
                "Explain Plugin".to_string()
            },
            profile: "none".to_string(),
            provider: ProviderId::Ollama,
            model: "mock-model".to_string(),
            context_mode: ContextMode::Legacy,
            context_scope: ChatContextScope::Automatic,
            scope_reason: ChatScopeReason::DefaultSetting,
            evidence_mode: ChatEvidenceMode::Optional,
            references: Vec::new(),
            history: Vec::new(),
            budgets: ChatBudgets::default(),
            generation: ChatGenerationOptions::default(),
            security_mode: ChatSecurityMode::ReadOnly,
            client: ChatClientMetadata {
                name: "vscode".to_string(),
                version: "0.2.0".to_string(),
                vscode_version: "1.125.0".to_string(),
                session_id: "session-test".to_string(),
                locale: "fr".to_string(),
                recent_run_ids: Vec::new(),
                previous_repository_state: None,
            },
            expected_protocols: ChatExpectedProtocols::default(),
            edit: None,
        }
    }

    async fn run(
        app: Option<&OpticCode>,
        request: ChatRequest,
    ) -> (ChatExecutionReport, Vec<crate::ChatProtocolEvent>) {
        let (events, mut receiver) = chat_event_channel(DEFAULT_CHAT_EVENT_CAPACITY).unwrap();
        let session = ChatProtocolSession {
            request_id: request.request_id.clone(),
            events,
            cancellation: opticcode_llm::CancellationToken::new(),
        };
        let policy_state = tempfile::tempdir().unwrap();
        let report = execute_chat(
            app,
            request,
            session,
            ChatRuntimeOptions {
                rag_index: PathBuf::from("missing-index"),
                verify_model: true,
                policy_state_root: Some(policy_state.path().to_path_buf()),
                proposal_state_root: Some(policy_state.path().to_path_buf()),
            },
        )
        .await
        .unwrap();
        let mut captured = Vec::new();
        while let Some(event) = receiver.recv().await {
            captured.push(event);
        }
        (report, captured)
    }

    #[tokio::test]
    async fn help_is_read_only_and_has_one_terminal() {
        let temp = fixture();
        let (report, events) = run(None, request(temp.path(), ChatCommand::Help)).await;
        assert_eq!(
            report.status,
            ChatExecutionStatus::Completed,
            "events: {events:#?}"
        );
        assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
        assert!(events
            .iter()
            .filter_map(|event| event.output_delta())
            .collect::<String>()
            .contains("POLICY-001"));
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            ChatProtocolEventPayload::RequestAccepted {
                requested_security_mode: ChatSecurityMode::ReadOnly,
                effective_security_mode: ChatSecurityMode::ReadOnly,
                policy_decision,
                policy_rule_id,
                ..
            } if policy_decision == "allow" && policy_rule_id == "analysis.context_read_only"
        )));
    }

    #[test]
    fn every_read_only_chat_command_is_evaluated_by_the_rust_policy_engine() {
        let temp = fixture();
        let policy_state = tempfile::tempdir().unwrap();
        let options = ChatRuntimeOptions {
            rag_index: PathBuf::from("missing-index"),
            verify_model: true,
            policy_state_root: Some(policy_state.path().to_path_buf()),
            proposal_state_root: Some(policy_state.path().to_path_buf()),
        };
        for command in [
            ChatCommand::Ask,
            ChatCommand::Plan,
            ChatCommand::Inspect,
            ChatCommand::Context,
            ChatCommand::Analyze,
            ChatCommand::Index,
            ChatCommand::Legacy,
            ChatCommand::Status,
            ChatCommand::Runs,
            ChatCommand::Help,
        ] {
            let authorization = authorize_chat_request(&request(temp.path(), command), &options)
                .unwrap_or_else(|error| panic!("/{command} was not policy-authorized: {error}"));
            assert_eq!(authorization.decision, "allow");
            assert_eq!(
                authorization.effective_security_mode,
                ChatSecurityMode::ReadOnly
            );
            assert!(matches!(
                authorization.rule_id.as_str(),
                "analysis.context_read_only" | "git.read_allowlist"
            ));
        }
    }

    #[tokio::test]
    async fn client_cannot_raise_chat_security_mode() {
        let temp = fixture();
        for mode in [
            ChatSecurityMode::WorktreeEdit,
            ChatSecurityMode::ApprovedApply,
        ] {
            let mut chat = request(temp.path(), ChatCommand::Fix);
            chat.security_mode = mode;
            let (report, events) = run(None, chat).await;
            assert_eq!(report.status, ChatExecutionStatus::Failed);
            assert!(events.iter().any(|event| matches!(
                &event.payload,
                ChatProtocolEventPayload::RequestAccepted {
                    requested_security_mode,
                    effective_security_mode: ChatSecurityMode::ReadOnly,
                    policy_decision,
                    ..
                } if *requested_security_mode == mode && policy_decision == "allow"
            )));
            assert!(events.iter().any(|event| matches!(
                &event.payload,
                ChatProtocolEventPayload::Failed { error }
                    if error.code == "security_mode_unavailable" && error.stage == "policy"
            )));
            assert!(!events.iter().any(|event| matches!(
                event.payload,
                ChatProtocolEventPayload::ReferencesResolving { .. }
            )));
        }
    }

    #[tokio::test]
    async fn ask_streams_through_the_existing_assistant_protocol() {
        let temp = fixture();
        let app = OpticCode::with_provider(Arc::new(MockProvider), "mock-model").unwrap();
        let mut chat = request(temp.path(), ChatCommand::Ask);
        chat.budgets.rag_hits = 0;
        let (report, events) = run(Some(&app), chat).await;
        assert_eq!(
            report.status,
            ChatExecutionStatus::Completed,
            "events: {events:#?}"
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| event.output_delta())
                .collect::<String>(),
            "mock answer"
        );
        assert!(events.iter().any(|event| matches!(
            event.payload,
            ChatProtocolEventPayload::ProviderStarted { .. }
        )));
    }

    #[tokio::test]
    async fn explicit_only_prompt_is_confined_to_the_current_reference() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("benchmarks/grounding-plugin");
        let mut chat = request(&workspace, ChatCommand::Ask);
        chat.prompt = concat!(
            "Lis uniquement le fichier plugin.yml joint.\n\n",
            "Retourne seulement :\n",
            "1. la liste exacte de ses cles de premier niveau ;\n",
            "2. la ligne exacte contenant api-version, si elle existe ;\n",
            "3. sinon ecris exactement : api-version absent.\n\n",
            "N'utilise aucune connaissance generale.\n",
            "Ne recommande aucune modification.\n",
            "Ne parle d'aucun autre fichier."
        )
        .to_string();
        chat.references.push(ChatReference {
            reference_id: "plugin-yml-only".to_string(),
            inclusion_reason: "attached by user".to_string(),
            target: ChatReferenceTarget::File {
                path: "src/main/resources/plugin.yml".to_string(),
            },
        });
        chat.history.push(ChatHistoryTurn {
            role: ChatHistoryRole::Assistant,
            content: "Inspect UnrelatedListener.java and run cargo benchmark commands.".to_string(),
            command: Some(ChatCommand::Ask),
            result_id: Some("stale-java-run".to_string()),
            source_scope: Some(ChatContextScope::Automatic),
            workspace_id: Some("workspace-test".to_string()),
            context_fingerprint: Some("stale-context".to_string()),
            grounding_status: Some("ungrounded".to_string()),
        });

        let (events, _receiver) = chat_event_channel(DEFAULT_CHAT_EVENT_CAPACITY).unwrap();
        let session = ChatProtocolSession {
            request_id: chat.request_id.clone(),
            events,
            cancellation: opticcode_llm::CancellationToken::new(),
        };
        let emitter = ChatEventEmitter::new(&session).unwrap();
        let prepared = prepare_request(&chat, &emitter).await.unwrap();

        assert!(prepared.prompt.contains("name: OutilsEvolutif"));
        assert!(!prepared.prompt.contains("[PROJECT_SUMMARY]"));
        assert!(!prepared.prompt.contains("[CHAT_HISTORY]"));
        assert!(!prepared.prompt.contains("UnrelatedListener.java"));
        assert!(!prepared.prompt.contains("cargo benchmark"));
    }

    #[tokio::test]
    async fn strict_yaml_question_uses_document_facts_without_a_provider() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("benchmarks/grounding-plugin");
        let mut chat = request(&workspace, ChatCommand::Ask);
        chat.prompt = concat!(
            "Lis uniquement le fichier plugin.yml joint.\n\n",
            "Retourne seulement :\n",
            "1. la liste exacte de ses cl\u{00e9}s de premier niveau ;\n",
            "2. la ligne exacte contenant api-version, si elle existe ;\n",
            "3. sinon \u{00e9}cris exactement : \u{00ab} api-version absent \u{00bb}.\n\n",
            "N'utilise aucune connaissance g\u{00e9}n\u{00e9}rale.\n",
            "Ne recommande aucune modification.\n",
            "Ne parle d'aucun autre fichier."
        )
        .to_string();
        chat.context_scope = ChatContextScope::ReferencesOnly;
        chat.evidence_mode = ChatEvidenceMode::Required;
        chat.references.push(ChatReference {
            reference_id: "plugin-yml-only".to_string(),
            inclusion_reason: "attached by user".to_string(),
            target: ChatReferenceTarget::File {
                path: "src/main/resources/plugin.yml".to_string(),
            },
        });

        let (report, events) = run(None, chat).await;
        assert_eq!(report.status, ChatExecutionStatus::Completed, "{events:#?}");
        assert_eq!(
            events
                .iter()
                .filter_map(crate::ChatProtocolEvent::output_delta)
                .collect::<String>(),
            "name\nmain\nversion\ncommands\napi-version absent"
        );
        assert!(!events.iter().any(|event| matches!(
            event.payload,
            ChatProtocolEventPayload::ProviderStarted { .. }
        )));
        assert!(events.iter().any(|event| matches!(
            event.payload,
            ChatProtocolEventPayload::DocumentInspectionCompleted { model_calls: 0, .. }
        )));
        let validation_sequence = events
            .iter()
            .find(|event| {
                matches!(
                    event.payload,
                    ChatProtocolEventPayload::GroundingValidationCompleted { .. }
                )
            })
            .map(|event| event.sequence)
            .unwrap();
        let answer_sequence = events
            .iter()
            .find(|event| matches!(event.payload, ChatProtocolEventPayload::TokenDelta { .. }))
            .map(|event| event.sequence)
            .unwrap();
        assert!(validation_sequence < answer_sequence);
        let grounding = events.iter().find_map(|event| match &event.payload {
            ChatProtocolEventPayload::Completed { summary } => summary.grounding.as_ref(),
            _ => None,
        });
        assert_eq!(
            grounding.map(|value| value.route),
            Some(GroundingRoute::DocumentFacts)
        );
        assert_eq!(grounding.map(|value| value.injected_references), Some(1));
        assert_eq!(grounding.map(|value| value.discovered_files), Some(0));
        assert_eq!(grounding.map(|value| value.rag_hits), Some(0));
    }

    #[tokio::test]
    async fn references_only_without_a_current_reference_fails_closed() {
        let temp = fixture();
        let mut chat = request(temp.path(), ChatCommand::Ask);
        chat.context_scope = ChatContextScope::ReferencesOnly;
        chat.evidence_mode = ChatEvidenceMode::Required;
        let (report, events) = run(None, chat).await;
        assert_eq!(report.status, ChatExecutionStatus::Failed);
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            ChatProtocolEventPayload::Failed { error }
                if error.code == "references_required"
        )));
    }

    #[tokio::test]
    async fn invalid_cross_file_evidence_is_refused_before_rendering() {
        let temp = fixture();
        let provider = Arc::new(StructuredMockProvider::new([serde_json::json!({
            "schema_version": 1,
            "answer": "Read UnrelatedListener.java.",
            "claims": [{
                "claim_id": "claim-1",
                "text": "UnrelatedListener.java is relevant.",
                "classification": "observed",
                "evidence": [{
                    "path": "src/main/java/test/UnrelatedListener.java",
                    "start_line": 1,
                    "end_line": 1,
                    "content_hash": "0".repeat(64)
                }]
            }],
            "missing_information": [],
            "used_general_knowledge": false
        })
        .to_string()]));
        let app = OpticCode::with_provider(provider.clone(), "mock-model").unwrap();
        let mut chat = request(temp.path(), ChatCommand::Ask);
        chat.prompt = "Use only this file. Describe the declared class.".to_string();
        chat.context_scope = ChatContextScope::ReferencesOnly;
        chat.evidence_mode = ChatEvidenceMode::Required;
        chat.references.push(ChatReference {
            reference_id: "plugin-java".to_string(),
            inclusion_reason: "attached by user".to_string(),
            target: ChatReferenceTarget::File {
                path: "src/main/java/test/Plugin.java".to_string(),
            },
        });
        let (report, events) = run(Some(&app), chat).await;
        assert_eq!(report.status, ChatExecutionStatus::Failed);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
        assert!(!events
            .iter()
            .any(|event| matches!(event.payload, ChatProtocolEventPayload::TokenDelta { .. })));
        assert!(events.iter().any(|event| matches!(
            event.payload,
            ChatProtocolEventPayload::TaskComplianceFailed { .. }
        )));
    }

    #[tokio::test]
    async fn grounded_llm_allows_only_one_format_correction() {
        let temp = fixture();
        let content =
            fs::read_to_string(temp.path().join("src/main/java/test/Plugin.java")).unwrap();
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        let valid = serde_json::json!({
            "schema_version": 1,
            "answer": "Plugin is the declared class.",
            "claims": [{
                "claim_id": "claim-1",
                "text": "Plugin is the declared class.",
                "classification": "observed",
                "evidence": [{
                    "path": "src/main/java/test/Plugin.java",
                    "start_line": 1,
                    "end_line": 2,
                    "content_hash": hash
                }]
            }],
            "missing_information": [],
            "used_general_knowledge": false
        })
        .to_string();
        let provider = Arc::new(StructuredMockProvider::new(["not-json".to_string(), valid]));
        let app = OpticCode::with_provider(provider.clone(), "mock-model").unwrap();
        let mut chat = request(temp.path(), ChatCommand::Ask);
        chat.prompt = "Use only this file. State the declared class name.".to_string();
        chat.context_scope = ChatContextScope::ReferencesOnly;
        chat.evidence_mode = ChatEvidenceMode::Required;
        chat.references.push(ChatReference {
            reference_id: "plugin-java".to_string(),
            inclusion_reason: "attached by user".to_string(),
            target: ChatReferenceTarget::File {
                path: "src/main/java/test/Plugin.java".to_string(),
            },
        });
        let (report, events) = run(Some(&app), chat).await;
        assert_eq!(report.status, ChatExecutionStatus::Completed, "{events:#?}");
        assert_eq!(provider.calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            events
                .iter()
                .filter_map(crate::ChatProtocolEvent::output_delta)
                .collect::<String>(),
            "Plugin is the declared class."
        );
    }

    #[tokio::test]
    async fn same_path_new_hash_and_parallel_references_are_isolated() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.yml"), "name: first\n").unwrap();
        fs::write(temp.path().join("b.yml"), "name: second\n").unwrap();
        let make_request = |path: &str, request_id: &str| {
            let mut chat = request(temp.path(), ChatCommand::Ask);
            chat.request_id = request_id.to_string();
            chat.prompt = "Use only the attached file. Is `name` present?".to_string();
            chat.context_scope = ChatContextScope::ReferencesOnly;
            chat.evidence_mode = ChatEvidenceMode::Required;
            chat.references.push(ChatReference {
                reference_id: format!("reference-{request_id}"),
                inclusion_reason: "attached by user".to_string(),
                target: ChatReferenceTarget::File {
                    path: path.to_string(),
                },
            });
            chat
        };
        let first_a = prepare_for_test(&make_request("a.yml", "request-a-first")).await;
        fs::write(temp.path().join("a.yml"), "name: changed\n").unwrap();
        let changed_a = prepare_for_test(&make_request("a.yml", "request-a-changed")).await;
        assert_ne!(first_a.manifest.fingerprint, changed_a.manifest.fingerprint);
        assert_ne!(first_a.prompt_fingerprint, changed_a.prompt_fingerprint);

        let request_a = make_request("a.yml", "request-a-parallel");
        let request_b = make_request("b.yml", "request-b-parallel");
        let (prepared_a, prepared_b) =
            tokio::join!(prepare_for_test(&request_a), prepare_for_test(&request_b));
        assert_eq!(prepared_a.manifest.entries[0].path, "a.yml");
        assert_eq!(prepared_b.manifest.entries[0].path, "b.yml");
        assert!(!prepared_a.prompt.contains("second"));
        assert!(!prepared_b.prompt.contains("changed"));
    }

    async fn prepare_for_test(request: &ChatRequest) -> PreparedRequest {
        let (events, _receiver) = chat_event_channel(DEFAULT_CHAT_EVENT_CAPACITY).unwrap();
        let session = ChatProtocolSession {
            request_id: request.request_id.clone(),
            events,
            cancellation: opticcode_llm::CancellationToken::new(),
        };
        let emitter = ChatEventEmitter::new(&session).unwrap();
        prepare_request(request, &emitter).await.unwrap()
    }

    #[tokio::test]
    async fn file_range_and_unicode_selection_are_materialized_safely() {
        let temp = fixture();
        let mut chat = request(temp.path(), ChatCommand::Help);
        chat.references.push(ChatReference {
            reference_id: "unicode-selection".to_string(),
            inclusion_reason: "selected by user".to_string(),
            target: ChatReferenceTarget::Selection {
                path: "src/main/java/test/Plugin.java".to_string(),
                range: ChatTextRange {
                    start: ChatTextPosition {
                        line: 1,
                        character: 38,
                    },
                    end: ChatTextPosition {
                        line: 1,
                        character: 41,
                    },
                },
            },
        });
        let (_, events) = run(None, chat).await;
        let resolved = events.iter().find_map(|event| match &event.payload {
            ChatProtocolEventPayload::ReferencesResolved { accepted, .. } => accepted.first(),
            _ => None,
        });
        assert_eq!(resolved.map(|item| item.kind.as_str()), Some("selection"));
    }

    #[tokio::test]
    async fn history_is_bounded_and_secrets_are_not_reinjected() {
        let temp = fixture();
        let mut chat = request(temp.path(), ChatCommand::Help);
        chat.history = (0..20)
            .map(|index| ChatHistoryTurn {
                role: ChatHistoryRole::User,
                content: if index == 19 {
                    "api_key=abcdefghijklmnopqrstuvwxyz123456".to_string()
                } else {
                    format!("turn {index}")
                },
                command: Some(ChatCommand::Ask),
                result_id: None,
                source_scope: Some(ChatContextScope::Automatic),
                workspace_id: Some("workspace-test".to_string()),
                context_fingerprint: Some(format!("context-{index}")),
                grounding_status: Some("grounded".to_string()),
            })
            .collect();
        let (_, events) = run(None, chat).await;
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            ChatProtocolEventPayload::Warning { message, .. }
                if message.contains("sensitive historical content")
        )));
    }

    #[tokio::test]
    async fn outside_sensitive_and_missing_references_are_rejected() {
        let temp = fixture();
        fs::write(temp.path().join(".env"), "TOKEN=secret-value").unwrap();
        let mut chat = request(temp.path(), ChatCommand::Help);
        for (id, path) in [
            ("outside", "../outside.txt"),
            ("sensitive", ".env"),
            ("missing", "missing.java"),
        ] {
            chat.references.push(ChatReference {
                reference_id: id.to_string(),
                inclusion_reason: "test".to_string(),
                target: ChatReferenceTarget::File {
                    path: path.to_string(),
                },
            });
        }
        let (_, events) = run(None, chat).await;
        let rejected = events.iter().find_map(|event| match &event.payload {
            ChatProtocolEventPayload::ReferencesResolved { rejected, .. } => Some(rejected),
            _ => None,
        });
        assert_eq!(rejected.map(Vec::len), Some(3));
    }

    #[test]
    fn edit_controls_are_command_scoped_and_fix_requires_a_task() {
        let temp = fixture();
        let mut fix = request(temp.path(), ChatCommand::Fix);
        fix.prompt.clear();
        assert_eq!(validate_request(&fix).unwrap_err().code, "prompt_required");

        let mut diff = request(temp.path(), ChatCommand::Diff);
        diff.edit = Some(crate::ChatEditControl {
            proposal_id: Some("plan-1".to_string()),
            native_confirmation: Some(crate::ChatNativeConfirmation {
                client: "opticcode-vscode".to_string(),
                confirmation_id: "confirmation-1".to_string(),
                approval_request_id: "apply-confirmation-1".to_string(),
            }),
            ..crate::ChatEditControl::default()
        });
        assert_eq!(
            validate_request(&diff).unwrap_err().code,
            "invalid_native_confirmation"
        );
    }
}
