use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Instant;

use anyhow::Result;
use opticcode_tools::git_state::capture_git_state;
use opticcode_tools::inspect_workspace;
use opticcode_tools::java_edits::{propose_java_edits, JavaEditOptions};
use opticcode_tools::java_index::{analyze_java_index, JavaIndexOptions};
use opticcode_tools::java_syntax::{analyze_java_syntax, JavaSyntaxOptions};
use opticcode_tools::rag::{
    inspect_sensitive_text, read_safe_workspace_file, MAX_SAFE_REFERENCE_FILE_BYTES,
};

use crate::chat_protocol::{
    ChatCommand, ChatCompletionSummary, ChatContextFile, ChatEventEmitter, ChatMetrics,
    ChatProtocolError, ChatProtocolEventPayload, ChatProtocolSession, ChatReference,
    ChatReferenceTarget, ChatRejectedReference, ChatRequest, ChatResolvedReference,
    ChatSecurityMode, ChatTextPosition, ChatTextRange, CHAT_PROTOCOL_ID,
    CHAT_PROTOCOL_SCHEMA_VERSION, MAX_CHAT_EVENT_TEXT_BYTES, MAX_CHAT_HISTORY_CHARS,
    MAX_CHAT_HISTORY_TOKENS, MAX_CHAT_HISTORY_TURNS, MAX_CHAT_OUTPUT_TOKENS, MAX_CHAT_PROMPT_CHARS,
    MAX_CHAT_REFERENCES, MAX_CHAT_REFERENCE_BYTES,
};
use crate::{
    assistant_event_channel, prepare_assistant_context, AskOptions, AssistantCommandReport,
    AssistantProtocolEventPayload, AssistantProtocolSession, ContextFallbackPolicy, ContextMode,
    OpticCode, PlanOptions, ASSISTANT_PROTOCOL_SCHEMA_VERSION, DEFAULT_ASSISTANT_EVENT_CAPACITY,
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
struct RuntimeFailure {
    code: &'static str,
    stage: &'static str,
    message: String,
    retriable: bool,
}

impl RuntimeFailure {
    fn new(code: &'static str, stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            stage,
            message: bounded_text(&message.into(), 8 * 1024),
            retriable: false,
        }
    }

    fn retriable(mut self, retriable: bool) -> Self {
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
struct PreparedRequest {
    workspace: PathBuf,
    prompt: String,
    references: Vec<ChatResolvedReference>,
    rejected: Vec<ChatRejectedReference>,
    warnings: Vec<String>,
    repository_state: String,
}

#[derive(Debug)]
struct ReferenceMaterial {
    summary: ChatResolvedReference,
    prompt_content: String,
}

#[derive(Debug)]
struct CommandOutcome {
    context_files: Vec<ChatContextFile>,
    used_context_mode: Option<ContextMode>,
    metrics: ChatMetrics,
    warnings: Vec<String>,
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
            emitter
                .send(ChatProtocolEventPayload::Metrics {
                    metrics: outcome.metrics.clone(),
                })
                .await?;
            let mut warnings = prepared.warnings.clone();
            warnings.extend(outcome.warnings);
            warnings.truncate(MAX_WARNING_COUNT);
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
            };
            emitter
                .send(ChatProtocolEventPayload::Completed { summary })
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
    emitter
        .send(ChatProtocolEventPayload::RequestAccepted {
            command: request.command,
            security_mode: request.security_mode,
        })
        .await
        .map_err(event_failure)?;
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
            run_unavailable_command(request.command, emitter, started).await?
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
    if request.security_mode != ChatSecurityMode::ReadOnly {
        return Err(RuntimeFailure::new(
            "security_mode_unavailable",
            "request_validation",
            "VSCODE-CHAT-001 accepts only read_only requests until POLICY-001 is active",
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
    emitter
        .send(ChatProtocolEventPayload::ReferencesResolving {
            count: request.references.len(),
        })
        .await
        .map_err(event_failure)?;

    let mut accepted_material = Vec::new();
    let mut rejected = Vec::new();
    let mut retained_bytes = 0usize;
    for reference in &request.references {
        match resolve_reference(&workspace, reference, request.budgets.max_reference_bytes) {
            Ok(material)
                if retained_bytes.saturating_add(material.summary.bytes)
                    <= request.budgets.max_reference_bytes =>
            {
                retained_bytes = retained_bytes.saturating_add(material.summary.bytes);
                accepted_material.push(material);
            }
            Ok(_) => rejected.push(rejected_reference(
                reference,
                "size.total_reference_budget",
                "reference would exceed the total attachment budget",
            )),
            Err(error) => rejected.push(rejected_reference(reference, error.code, &error.message)),
        }
    }
    let references = accepted_material
        .iter()
        .map(|material| material.summary.clone())
        .collect::<Vec<_>>();
    emitter
        .send(ChatProtocolEventPayload::ReferencesResolved {
            accepted: references.clone(),
            rejected: rejected.clone(),
        })
        .await
        .map_err(event_failure)?;

    let mut warnings = rejected
        .iter()
        .map(|item| format!("{}: {}", item.rule_id, item.reason))
        .collect::<Vec<_>>();
    let (history, history_warnings) = bounded_history(request);
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
    let explicit_references = render_reference_material(&accepted_material);
    let prompt = format!(
        concat!(
            "[PROJECT_SUMMARY]\n{}\n\n",
            "[CHAT_HISTORY]\n{}\n\n",
            "[CURRENT_REQUEST]\n{}\n\n",
            "[EXPLICIT_REFERENCES]\n{}"
        ),
        project_summary, history, request.prompt, explicit_references
    );
    if estimate_tokens(&prompt) > request.budgets.max_prompt_tokens {
        return Err(RuntimeFailure::new(
            "prompt_budget_exceeded",
            "prompt_preparation",
            "bounded history and explicit references exceed the configured prompt budget",
        ));
    }
    Ok(PreparedRequest {
        workspace,
        prompt,
        references,
        rejected,
        warnings,
        repository_state,
    })
}

fn resolve_reference(
    workspace: &Path,
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
            let content = match target {
                ChatReferenceTarget::Range { range, .. }
                | ChatReferenceTarget::Selection { range, .. } => {
                    extract_utf16_range(&file.content, *range)?
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
                        extract_utf16_range(&file.content, *range)?
                    } else {
                        extract_symbol_context(&file.content, symbol)?
                    }
                }
                ChatReferenceTarget::Finding {
                    range: Some(range), ..
                } => extract_utf16_range(&file.content, *range)?,
                _ => file.content.clone(),
            };
            if content.len() > MAX_SAFE_REFERENCE_FILE_BYTES as usize {
                return Err(RuntimeFailure::new(
                    "size.reference_material",
                    "reference_resolution",
                    "materialized reference exceeds the hard byte limit",
                ));
            }
            Ok(ReferenceMaterial {
                summary: ChatResolvedReference {
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
                    content_hash: Some(blake3::hash(content.as_bytes()).to_hex().to_string()),
                },
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
        },
        prompt_content,
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

fn bounded_history(request: &ChatRequest) -> (String, Vec<String>) {
    if request.history.is_empty() {
        return ("none".to_string(), Vec::new());
    }
    let mut selected = Vec::new();
    let mut chars = 0usize;
    let mut tokens = 0usize;
    let mut warnings = Vec::new();
    for turn in request.history.iter().rev() {
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
    (selected.join("\n\n"), warnings)
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
    let metrics = ChatMetrics {
        preparation_ms: report.preparation_duration_us.saturating_add(999) / 1_000,
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
    };
    Ok(CommandOutcome {
        context_files,
        used_context_mode: report.used_context_mode,
        metrics,
        warnings: report.warnings,
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
            "- write/apply: unavailable until POLICY-001/CHAT-EDIT-001"
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
        "OpticCode Local is a read-only project assistant in this milestone.\n\n",
        "- `/ask`: answer with bounded project, RAG, history, and explicit references\n",
        "- `/plan`: produce a plan without writing\n",
        "- `/context`: inspect legacy/symbol context\n",
        "- `/analyze`, `/index`, `/legacy`: run read-only Java analysis\n",
        "- `/status`, `/runs`: inspect local state\n",
        "- `/fix`, `/verify`, `/diff`, `/apply`, `/rollback`: unavailable until POLICY-001/CHAT-EDIT-001\n\n",
        "Attached files are never implicit write permission. Sensitive files, paths outside the workspace, and symlinks/junctions are refused."
    );
    emit_text(emitter, markdown).await?;
    Ok(empty_outcome(started))
}

async fn run_unavailable_command(
    command: ChatCommand,
    emitter: &ChatEventEmitter,
    started: Instant,
) -> std::result::Result<CommandOutcome, RuntimeFailure> {
    emitter
        .send(ChatProtocolEventPayload::Warning {
            code: "feature_unavailable".to_string(),
            message: format!(
                "/{command} is unavailable until POLICY-001/CHAT-EDIT-001 is fully active"
            ),
        })
        .await
        .map_err(event_failure)?;
    emit_text(
        emitter,
        &format!(
            "`/{command}` is unavailable until POLICY-001/CHAT-EDIT-001. No file, worktree, process, or Git state was changed."
        ),
    )
    .await?;
    Ok(CommandOutcome {
        warnings: vec![format!("/{command} is not active in read_only mode")],
        ..empty_outcome(started)
    })
}

async fn emit_text(
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

fn extract_utf16_range(
    content: &str,
    range: ChatTextRange,
) -> std::result::Result<String, RuntimeFailure> {
    let start = position_to_byte(content, range.start)?;
    let end = position_to_byte(content, range.end)?;
    if start > end {
        return Err(RuntimeFailure::new(
            "reference.invalid_range",
            "reference_resolution",
            "range start is after range end",
        ));
    }
    Ok(content[start..end].to_string())
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
    }
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
    use std::fs;
    use std::sync::Arc;

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

    fn generation(request: &GenerationRequest) -> GenerationResult {
        GenerationResult {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            provider: ProviderId::Ollama,
            model: request.model.clone(),
            output: "mock answer".to_string(),
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
        let report = execute_chat(
            app,
            request,
            session,
            ChatRuntimeOptions {
                rag_index: PathBuf::from("missing-index"),
                verify_model: true,
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

    #[tokio::test]
    async fn edit_commands_are_explicitly_unavailable_before_policy() {
        let temp = fixture();
        let (report, events) = run(None, request(temp.path(), ChatCommand::Apply)).await;
        assert_eq!(report.status, ChatExecutionStatus::Completed);
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            ChatProtocolEventPayload::Warning { code, .. } if code == "feature_unavailable"
        )));
    }
}
