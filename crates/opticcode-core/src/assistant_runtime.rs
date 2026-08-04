use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use opticcode_llm::{
    event_channel, CancellationToken, GenerateMetrics, GenerationRequest, GenerationResult,
    HealthRequest, LlmProtocolEvent, LlmProvider, ProviderError, ProviderErrorKind,
    ProviderGenerationOptions, MAX_REQUEST_ID_BYTES,
};
use serde::{Deserialize, Serialize};

use crate::protocol::{AssistantCompletionSummary, AssistantEventEmitter};
use crate::{
    build_plan_prompt, build_prompt, load_memory_for_workspace, load_profile_for_workspace,
    load_rag_context, prepare_assistant_context, AssistantProtocolEventPayload,
    AssistantProtocolSession, ContextFallbackPolicy, ContextMode, ContextPreparation,
    MemoryContext, ProfileContext, RagContext,
};

pub const ASSISTANT_RUN_SCHEMA_VERSION: u32 = 1;
pub const ASSISTANT_PROMPT_VERSION: &str = "opticcode-assistant-prompt-v2";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AssistantCommandKind {
    Ask,
    Plan,
}

impl AssistantCommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Plan => "plan",
        }
    }

    fn brief_default_tokens(self) -> u32 {
        match self {
            Self::Ask => 240,
            Self::Plan => 320,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantGenerationConfiguration {
    pub provider: &'static str,
    pub endpoint: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub seed: Option<i64>,
    pub max_generated_tokens: Option<u32>,
    pub http_timeout_ms: u64,
    pub absolute_determinism_guaranteed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantPromptReport {
    pub version: &'static str,
    pub bytes: usize,
    pub chars: usize,
    pub estimated_tokens: usize,
    pub token_estimator: &'static str,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantGenerationMetrics {
    pub client_ms: u64,
    pub ollama_total_ms: Option<u64>,
    pub ollama_load_ms: Option<u64>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_ms: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub generation_ms: Option<u64>,
    pub generated_tokens_per_second: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantRagHitReport {
    pub source: String,
    pub chunk_id: String,
    pub score: usize,
    pub weighted_score: usize,
    pub matched_queries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantRagReport {
    pub enabled: bool,
    pub validated_active_v2: bool,
    pub queries: Vec<String>,
    pub hits: Vec<AssistantRagHitReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantStructuredError {
    pub code: String,
    pub stage: String,
    pub message: String,
    pub context_mode: Option<ContextMode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantRunReport {
    pub context_mode: ContextMode,
    pub generated: bool,
    pub skipped_reason: Option<String>,
    pub prompt: AssistantPromptReport,
    pub metrics: Option<AssistantGenerationMetrics>,
    pub response: Option<String>,
    pub error: Option<AssistantStructuredError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantCommandReport {
    pub schema_version: u32,
    pub command: AssistantCommandKind,
    pub request: String,
    pub success: bool,
    pub provider: &'static str,
    pub model: String,
    pub requested_context_mode: ContextMode,
    pub used_context_mode: Option<ContextMode>,
    pub analysis_complete: bool,
    pub double_generation_authorized: bool,
    pub preparation_duration_us: u64,
    pub generation: AssistantGenerationConfiguration,
    pub context: ContextPreparation,
    pub rag: AssistantRagReport,
    pub runs: Vec<AssistantRunReport>,
    pub warnings: Vec<String>,
    pub errors: Vec<AssistantStructuredError>,
}

impl AssistantCommandReport {
    pub fn generated_run(&self) -> Option<&AssistantRunReport> {
        self.runs.iter().find(|run| run.generated)
    }
}

// Kept private to prevent callers from treating serialized millisecond metrics as the
// original high-resolution provider metrics.
pub(crate) struct AssistantExecutionOutput {
    pub report: AssistantCommandReport,
    pub raw_metrics: Vec<(ContextMode, GenerateMetrics)>,
}

pub(crate) struct AssistantExecutionOptions<'a> {
    pub command: AssistantCommandKind,
    pub workspace: &'a Path,
    pub request: &'a str,
    pub profile: Option<&'a str>,
    pub include_memory: bool,
    pub include_rag: bool,
    pub rag_index: &'a Path,
    pub rag_limit: usize,
    pub brief: bool,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub seed: Option<i64>,
    pub context_mode: ContextMode,
    pub fallback_policy: ContextFallbackPolicy,
    pub compare_generate: bool,
    pub verify_model: bool,
    pub keep_alive: Option<String>,
    pub http_timeout: Duration,
    pub protocol: Option<AssistantProtocolSession>,
}

pub(crate) async fn execute_assistant(
    llm: &dyn LlmProvider,
    model: &str,
    options: AssistantExecutionOptions<'_>,
) -> Result<AssistantExecutionOutput> {
    let emitter = options
        .protocol
        .as_ref()
        .map(AssistantEventEmitter::new)
        .transpose()?;
    if let Some(emitter) = &emitter {
        emitter
            .send(AssistantProtocolEventPayload::Started {
                command: options.command,
                provider: llm.id(),
                model: model.to_string(),
                requested_context_mode: options.context_mode,
            })
            .await?;
    }

    let output = execute_assistant_inner(llm, model, &options, emitter.as_ref()).await;
    match output {
        Ok(output) => {
            if let Some(emitter) = &emitter {
                let payload = if output.report.success {
                    AssistantProtocolEventPayload::Completed {
                        report_schema_version: output.report.schema_version,
                        generated_runs: output
                            .report
                            .runs
                            .iter()
                            .filter(|run| run.generated)
                            .count(),
                        summary: Some(Box::new(AssistantCompletionSummary::from(&output.report))),
                    }
                } else if report_was_cancelled(&output.report) {
                    AssistantProtocolEventPayload::Cancelled {
                        errors: output.report.errors.clone(),
                    }
                } else {
                    AssistantProtocolEventPayload::Failed {
                        errors: output.report.errors.clone(),
                    }
                };
                emitter.send(payload).await?;
            }
            Ok(output)
        }
        Err(error) => {
            if let Some(emitter) = &emitter {
                let structured = AssistantStructuredError {
                    code: if options
                        .protocol
                        .as_ref()
                        .is_some_and(|session| session.cancellation.is_cancelled())
                    {
                        "request_cancelled".to_string()
                    } else {
                        "command_failed".to_string()
                    },
                    stage: "assistant_runtime".to_string(),
                    message: format!("{error:#}"),
                    context_mode: None,
                };
                let payload = if structured.code == "request_cancelled" {
                    AssistantProtocolEventPayload::Cancelled {
                        errors: vec![structured],
                    }
                } else {
                    AssistantProtocolEventPayload::Failed {
                        errors: vec![structured],
                    }
                };
                emitter.send(payload).await?;
            }
            Err(error)
        }
    }
}

async fn execute_assistant_inner(
    llm: &dyn LlmProvider,
    model: &str,
    options: &AssistantExecutionOptions<'_>,
    emitter: Option<&AssistantEventEmitter>,
) -> Result<AssistantExecutionOutput> {
    validate_execution_options(model, options)?;
    let cancellation = options
        .protocol
        .as_ref()
        .map(|session| session.cancellation.clone())
        .unwrap_or_default();
    if cancellation.is_cancelled() {
        bail!("assistant request was cancelled before context preparation");
    }
    let preparation_started = Instant::now();
    let context = prepare_assistant_context(
        options.workspace,
        options.request,
        options.context_mode,
        options.fallback_policy,
    )?;
    let profile = load_profile_for_workspace(options.workspace, options.profile)?;
    let memory = if options.include_memory {
        load_memory_for_workspace(options.workspace, options.profile)?
    } else {
        MemoryContext::default()
    };
    let rag = if options.include_rag {
        load_rag_context(options.rag_index, options.request, options.rag_limit)?
    } else {
        RagContext::default()
    };
    let max_generated_tokens = options.max_tokens.or_else(|| {
        options
            .brief
            .then(|| options.command.brief_default_tokens())
    });
    let mut warnings = context
        .fallback
        .as_ref()
        .map(|fallback| vec![fallback.warning.clone()])
        .unwrap_or_default();
    if options.context_mode == ContextMode::Compare && !options.compare_generate {
        warnings.push(
            "context comparison completed without model generation; use --compare-generate to explicitly authorize two calls"
                .to_string(),
        );
    }
    if options.compare_generate {
        warnings.push(
            "two model generations were explicitly authorized; both use identical generation settings"
                .to_string(),
        );
    }

    let generation = AssistantGenerationConfiguration {
        provider: llm.id().as_str(),
        endpoint: llm.endpoint().to_string(),
        model: model.to_string(),
        temperature: options.temperature,
        seed: options.seed,
        max_generated_tokens,
        http_timeout_ms: duration_ms_ceil(options.http_timeout),
        absolute_determinism_guaranteed: false,
    };
    let modes = requested_run_modes(&context, options.context_mode);
    let mut prompts = Vec::new();
    let mut runs = Vec::new();
    for mode in modes {
        let variant = context
            .variant(mode)
            .ok_or_else(|| anyhow::anyhow!("prepared context variant `{mode}` is missing"))?;
        let prompt = compose_prompt(
            options.command,
            options.request,
            &variant.prompt_context,
            profile.as_ref(),
            &memory,
            &rag,
            options.brief,
        );
        let skipped_reason = (!variant.report.usable_for_generation).then(|| {
            format!(
                "context rejected: {}",
                variant
                    .report
                    .rejection_reasons
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        runs.push(AssistantRunReport {
            context_mode: mode,
            generated: false,
            skipped_reason,
            prompt: prompt_report(&prompt),
            metrics: None,
            response: None,
            error: None,
        });
        prompts.push(prompt);
    }

    let rag_report = rag_report(options.include_rag, &rag);
    let preparation_duration_us = duration_us(preparation_started.elapsed());
    let mut report = AssistantCommandReport {
        schema_version: ASSISTANT_RUN_SCHEMA_VERSION,
        command: options.command,
        request: options.request.to_string(),
        success: false,
        provider: llm.id().as_str(),
        model: model.to_string(),
        requested_context_mode: options.context_mode,
        used_context_mode: context.used_mode,
        analysis_complete: context.analysis_complete,
        double_generation_authorized: options.compare_generate,
        preparation_duration_us,
        generation,
        context,
        rag: rag_report,
        runs,
        warnings,
        errors: Vec::new(),
    };

    if let Some(emitter) = emitter {
        emitter
            .send(AssistantProtocolEventPayload::ContextPrepared {
                requested_context_mode: report.requested_context_mode,
                used_context_mode: report.used_context_mode,
                analysis_complete: report.analysis_complete,
                fallback_applied: report.context.fallback.is_some(),
                variant_count: report.context.variants.len(),
            })
            .await?;
    }

    if cancellation.is_cancelled() {
        attach_error(
            &mut report,
            AssistantStructuredError {
                code: "request_cancelled".to_string(),
                stage: "context_preparation".to_string(),
                message: "assistant request was cancelled after context preparation".to_string(),
                context_mode: None,
            },
        );
        return Ok(AssistantExecutionOutput {
            report,
            raw_metrics: Vec::new(),
        });
    }

    if options.context_mode == ContextMode::Compare && !options.compare_generate {
        report.success = true;
        return Ok(AssistantExecutionOutput {
            report,
            raw_metrics: Vec::new(),
        });
    }

    if report.used_context_mode.is_none() {
        let error = context_rejected_error(&report.context, ContextMode::Symbol);
        attach_error(&mut report, error);
        return Ok(AssistantExecutionOutput {
            report,
            raw_metrics: Vec::new(),
        });
    }
    if options.context_mode == ContextMode::Compare
        && report
            .context
            .variant(ContextMode::Symbol)
            .is_some_and(|variant| !variant.report.usable_for_generation)
    {
        let error = context_rejected_error(&report.context, ContextMode::Symbol);
        attach_error(&mut report, error);
        return Ok(AssistantExecutionOutput {
            report,
            raw_metrics: Vec::new(),
        });
    }

    if options.verify_model {
        // Report schema v1 keeps its Ollama labels; provider events carry the neutral error.
        let health = llm.health(HealthRequest {
            model: Some(model.to_string()),
            timeout_ms: duration_ms_ceil(options.http_timeout),
            ..HealthRequest::default()
        });
        tokio::pin!(health);
        let health = tokio::select! {
            _ = cancellation.cancelled() => {
                Err(ProviderError::cancelled(llm.id(), "provider_preflight"))
            }
            health = &mut health => health,
        };
        match health {
            Err(error) if error.kind == ProviderErrorKind::Cancelled => {
                attach_error(
                    &mut report,
                    AssistantStructuredError {
                        code: "request_cancelled".to_string(),
                        stage: error.stage,
                        message: "assistant request was cancelled during provider preflight"
                            .to_string(),
                        context_mode: None,
                    },
                );
                return Ok(AssistantExecutionOutput {
                    report,
                    raw_metrics: Vec::new(),
                });
            }
            Ok(health) if health.model_available == Some(true) => {}
            Ok(_) => {
                attach_error(
                    &mut report,
                    AssistantStructuredError {
                        code: "model_unavailable".to_string(),
                        stage: "ollama_preflight".to_string(),
                        message: format!(
                            "configured model `{model}` is not present in the local Ollama inventory"
                        ),
                        context_mode: None,
                    },
                );
                return Ok(AssistantExecutionOutput {
                    report,
                    raw_metrics: Vec::new(),
                });
            }
            Err(error) => {
                attach_error(
                    &mut report,
                    AssistantStructuredError {
                        code: "ollama_unavailable".to_string(),
                        stage: "ollama_preflight".to_string(),
                        message: format!("local Ollama preflight failed: {error}"),
                        context_mode: None,
                    },
                );
                return Ok(AssistantExecutionOutput {
                    report,
                    raw_metrics: Vec::new(),
                });
            }
        }
    }

    let generation_options = ProviderGenerationOptions {
        max_output_tokens: max_generated_tokens,
        temperature: options.temperature,
        seed: options.seed,
        keep_alive: options.keep_alive.clone(),
        timeout_ms: duration_ms_ceil(options.http_timeout),
    };
    let mut raw_metrics = Vec::new();
    let request_id = options
        .protocol
        .as_ref()
        .map(|session| session.request_id.clone())
        .unwrap_or_else(crate::generated_request_id);
    for (index, prompt) in prompts.iter().enumerate() {
        if report.runs[index].skipped_reason.is_some() {
            continue;
        }
        let mode = report.runs[index].context_mode;
        let mut request =
            GenerationRequest::new(provider_request_id(&request_id, mode), model, prompt);
        request.options = generation_options.clone();
        let generated = if let Some(emitter) = emitter {
            stream_provider_generation(llm, request, mode, emitter, cancellation.clone()).await
        } else {
            llm.generate(request, cancellation.clone()).await
        };
        match generated {
            Ok(generated) => {
                report.runs[index].generated = true;
                report.runs[index].response = Some(generated.output.clone());
                report.runs[index].metrics = Some(metrics_report(&generated));
                raw_metrics.push((mode, legacy_metrics(&generated, options.keep_alive.clone())));
            }
            Err(error) => {
                let (code, stage, message) = if error.kind == ProviderErrorKind::Cancelled {
                    (
                        "generation_cancelled",
                        error.stage.clone(),
                        format!("local LLM provider generation failed: {error}"),
                    )
                } else {
                    (
                        "generation_failed",
                        "ollama_generate".to_string(),
                        format!("local Ollama generation failed: {error}"),
                    )
                };
                let structured = AssistantStructuredError {
                    code: code.to_string(),
                    stage,
                    message,
                    context_mode: Some(mode),
                };
                report.runs[index].error = Some(structured.clone());
                report.errors.push(structured);
            }
        }
    }
    report.success = report.errors.is_empty()
        && report
            .runs
            .iter()
            .all(|run| run.generated && run.error.is_none());
    Ok(AssistantExecutionOutput {
        report,
        raw_metrics,
    })
}

fn validate_execution_options(model: &str, options: &AssistantExecutionOptions<'_>) -> Result<()> {
    if model.trim().is_empty() {
        bail!("LLM model name must not be empty");
    }
    if options.request.trim().is_empty() {
        bail!("assistant request must not be empty");
    }
    if options.max_tokens == Some(0) {
        bail!("maximum generated tokens must be greater than zero");
    }
    if options
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        bail!("temperature must be a finite value between 0 and 2");
    }
    if options.compare_generate && options.context_mode != ContextMode::Compare {
        bail!("double generation requires context mode `compare`");
    }
    Ok(())
}

fn requested_run_modes(context: &ContextPreparation, requested: ContextMode) -> Vec<ContextMode> {
    if requested == ContextMode::Compare {
        vec![ContextMode::Legacy, ContextMode::Symbol]
    } else if let Some(used) = context.used_mode {
        vec![used]
    } else {
        vec![requested]
    }
}

fn provider_request_id(assistant_request_id: &str, mode: ContextMode) -> String {
    let suffix = format!(":{}", mode.as_str());
    if assistant_request_id.len().saturating_add(suffix.len()) <= MAX_REQUEST_ID_BYTES {
        return format!("{assistant_request_id}{suffix}");
    }
    let digest = blake3::hash(assistant_request_id.as_bytes())
        .to_hex()
        .to_string();
    format!("assistant-{}{suffix}", &digest[..24])
}

fn compose_prompt(
    command: AssistantCommandKind,
    request: &str,
    project_context: &str,
    profile: Option<&ProfileContext>,
    memory: &MemoryContext,
    rag: &RagContext,
    brief: bool,
) -> String {
    match command {
        AssistantCommandKind::Ask => {
            build_prompt(request, project_context, profile, memory, rag, brief)
        }
        AssistantCommandKind::Plan => {
            build_plan_prompt(request, project_context, profile, memory, rag, brief)
        }
    }
}

fn prompt_report(prompt: &str) -> AssistantPromptReport {
    AssistantPromptReport {
        version: ASSISTANT_PROMPT_VERSION,
        bytes: prompt.len(),
        chars: prompt.chars().count(),
        estimated_tokens: estimate_tokens(prompt),
        token_estimator: "estimate:ceil_unicode_chars_div_4",
        content_hash: blake3::hash(prompt.as_bytes()).to_hex().to_string(),
    }
}

async fn stream_provider_generation(
    provider: &dyn LlmProvider,
    request: GenerationRequest,
    context_mode: ContextMode,
    emitter: &AssistantEventEmitter,
    cancellation: CancellationToken,
) -> std::result::Result<GenerationResult, ProviderError> {
    let mut validation = ProviderEventValidationState::new(&request);
    let (events, mut receiver) = event_channel(64).map_err(|error| {
        ProviderError::new(
            provider.id(),
            ProviderErrorKind::InvalidConfiguration,
            "provider_event_channel",
            false,
            error.to_string(),
        )
    })?;
    let generation = provider.stream(request, events, cancellation.clone());
    tokio::pin!(generation);

    loop {
        tokio::select! {
            result = &mut generation => {
                while let Ok(event) = receiver.try_recv() {
                    validate_and_forward_provider_event(
                        provider,
                        context_mode,
                        event,
                        &mut validation,
                        emitter,
                        &cancellation,
                    ).await?;
                }
                return validate_provider_stream_completion(provider, result, &validation);
            }
            event = receiver.recv() => {
                let Some(event) = event else {
                    let result = (&mut generation).await;
                    return validate_provider_stream_completion(provider, result, &validation);
                };
                validate_and_forward_provider_event(
                    provider,
                    context_mode,
                    event,
                    &mut validation,
                    emitter,
                    &cancellation,
                ).await?;
            }
        }
    }
}

struct ProviderEventValidationState {
    expected_request_id: String,
    expected_model: String,
    next_sequence: u64,
    terminal_count: usize,
}

impl ProviderEventValidationState {
    fn new(request: &GenerationRequest) -> Self {
        Self {
            expected_request_id: request.request_id.clone(),
            expected_model: request.model.clone(),
            next_sequence: 0,
            terminal_count: 0,
        }
    }
}

async fn validate_and_forward_provider_event(
    provider: &dyn LlmProvider,
    context_mode: ContextMode,
    event: LlmProtocolEvent,
    validation: &mut ProviderEventValidationState,
    emitter: &AssistantEventEmitter,
    cancellation: &CancellationToken,
) -> std::result::Result<(), ProviderError> {
    if validation.terminal_count > 0 {
        cancellation.cancel();
        return Err(provider_protocol_error(
            provider,
            "provider emitted an event after its terminal event",
        ));
    }
    if let Err(error) = event.validate(
        provider.id(),
        &validation.expected_request_id,
        &validation.expected_model,
        validation.next_sequence,
    ) {
        cancellation.cancel();
        return Err(error);
    }
    validation.next_sequence = validation.next_sequence.saturating_add(1);
    if event.is_terminal() {
        validation.terminal_count += 1;
    }
    if let Err(error) = emitter
        .send(AssistantProtocolEventPayload::ProviderEvent {
            context_mode,
            event: Box::new(event),
        })
        .await
    {
        cancellation.cancel();
        return Err(ProviderError::new(
            provider.id(),
            ProviderErrorKind::EventSinkClosed,
            "assistant_event_forwarding",
            false,
            format!("failed to forward provider event: {error:#}"),
        ));
    }
    Ok(())
}

fn validate_provider_stream_completion(
    provider: &dyn LlmProvider,
    result: std::result::Result<GenerationResult, ProviderError>,
    validation: &ProviderEventValidationState,
) -> std::result::Result<GenerationResult, ProviderError> {
    if validation.terminal_count != 1 {
        return Err(provider_protocol_error(
            provider,
            format!(
                "provider stream expected exactly one terminal event, received {}",
                validation.terminal_count
            ),
        ));
    }
    result
}

fn provider_protocol_error(
    provider: &dyn LlmProvider,
    message: impl Into<String>,
) -> ProviderError {
    ProviderError::new(
        provider.id(),
        ProviderErrorKind::Protocol,
        "provider_event_validation",
        false,
        message,
    )
}

fn metrics_report(result: &GenerationResult) -> AssistantGenerationMetrics {
    let generated_tokens_per_second =
        match (result.usage.generated_tokens, result.timings.generation_ms) {
            (Some(tokens), Some(milliseconds)) if milliseconds > 0 => {
                Some(tokens as f64 * 1_000.0 / milliseconds as f64)
            }
            _ => None,
        };
    AssistantGenerationMetrics {
        client_ms: result.timings.client_ms,
        ollama_total_ms: result.timings.provider_total_ms,
        ollama_load_ms: result.timings.load_ms,
        prompt_eval_count: result.usage.prompt_tokens,
        prompt_eval_ms: result.timings.prompt_eval_ms,
        generated_tokens: result.usage.generated_tokens,
        generation_ms: result.timings.generation_ms,
        generated_tokens_per_second,
    }
}

fn legacy_metrics(result: &GenerationResult, keep_alive: Option<String>) -> GenerateMetrics {
    GenerateMetrics {
        client_duration: Duration::from_millis(result.timings.client_ms),
        prompt_chars: result.prompt_chars,
        ollama_total_duration: result.timings.provider_total_ms.map(Duration::from_millis),
        ollama_load_duration: result.timings.load_ms.map(Duration::from_millis),
        keep_alive,
        prompt_eval_count: result.usage.prompt_tokens,
        prompt_eval_duration: result.timings.prompt_eval_ms.map(Duration::from_millis),
        eval_count: result.usage.generated_tokens,
        eval_duration: result.timings.generation_ms.map(Duration::from_millis),
    }
}

fn report_was_cancelled(report: &AssistantCommandReport) -> bool {
    report
        .errors
        .iter()
        .any(|error| error.code == "generation_cancelled" || error.code == "request_cancelled")
}

fn rag_report(enabled: bool, rag: &RagContext) -> AssistantRagReport {
    AssistantRagReport {
        enabled,
        validated_active_v2: enabled,
        queries: rag.queries.clone(),
        hits: rag
            .hits
            .iter()
            .map(|hit| AssistantRagHitReport {
                source: hit.source.clone(),
                chunk_id: hit.chunk_id.clone(),
                score: hit.score,
                weighted_score: hit.weighted_score,
                matched_queries: hit.matched_queries.clone(),
            })
            .collect(),
    }
}

fn context_rejected_error(
    context: &ContextPreparation,
    mode: ContextMode,
) -> AssistantStructuredError {
    let reasons = context
        .variant(mode)
        .map(|variant| {
            variant
                .report
                .rejection_reasons
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|reasons| !reasons.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    AssistantStructuredError {
        code: "context_rejected".to_string(),
        stage: "context_preparation".to_string(),
        message: format!("symbol context is not safe for generation: {reasons}"),
        context_mode: Some(mode),
    }
}

fn attach_error(report: &mut AssistantCommandReport, error: AssistantStructuredError) {
    if let Some(mode) = error.context_mode {
        if let Some(run) = report.runs.iter_mut().find(|run| run.context_mode == mode) {
            run.error = Some(error.clone());
        }
    }
    report.errors.push(error);
    report.success = false;
}

fn estimate_tokens(value: &str) -> usize {
    value.chars().count().saturating_add(3) / 4
}

fn duration_us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn duration_ms_ceil(duration: Duration) -> u64 {
    let nanos = duration.as_nanos();
    nanos
        .saturating_add(999_999)
        .checked_div(1_000_000)
        .unwrap_or(u128::from(u64::MAX))
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use async_trait::async_trait;
    use opticcode_llm::{
        CancellationToken, EventSink, FinishReason, GenerationRequest, GenerationResult,
        GenerationTimings, GenerationUsage, HealthReport, HealthRequest, HealthStatus,
        LlmProtocolEvent, LlmProtocolEventPayload, LlmProvider, ModelInfo, ProviderCapabilities,
        ProviderError, ProviderId, LLM_PROTOCOL_SCHEMA_VERSION,
    };

    use crate::{
        assistant_event_channel, AskOptions, AssistantProtocolEventPayload,
        AssistantProtocolSession, ContextFallbackPolicy, ContextMode, OpticCode, PlanOptions,
    };

    const TAGS: &str =
        r#"{"models":[{"name":"qwen2.5-coder:14b","model":"qwen2.5-coder:14b","size":1}]}"#;
    const GENERATE: &str = r#"{"response":"mock response","done":true,"total_duration":1000000,"load_duration":1000,"prompt_eval_count":20,"prompt_eval_duration":2000,"eval_count":5,"eval_duration":3000}"#;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-index-mini")
    }

    fn app(responses: Vec<&'static str>) -> (OpticCode, Receiver<String>) {
        let (url, requests) = spawn_mock(responses);
        let app = OpticCode::try_new(url, "qwen2.5-coder:14b")
            .unwrap()
            .with_http_timeout(Duration::from_secs(2))
            .unwrap();
        (app, requests)
    }

    fn ask_options(prompt: &str, mode: ContextMode) -> AskOptions {
        AskOptions {
            workspace: fixture(),
            prompt: prompt.to_string(),
            profile: None,
            include_memory: false,
            include_rag: false,
            rag_index: PathBuf::from("unused"),
            rag_limit: 4,
            brief: false,
            max_tokens: Some(32),
            temperature: Some(0.0),
            seed: Some(7),
            context_mode: mode,
            fallback_policy: ContextFallbackPolicy::Legacy,
            compare_generate: false,
            verify_model: true,
        }
    }

    #[tokio::test]
    async fn ask_legacy_generates_one_measured_response() {
        let (app, requests) = app(vec![TAGS, GENERATE]);

        let report = app
            .ask_with_report(ask_options("Locate Helpers#ping().", ContextMode::Legacy))
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.used_context_mode, Some(ContextMode::Legacy));
        assert_eq!(report.runs.len(), 1);
        assert_eq!(
            report.runs[0].metrics.as_ref().unwrap().prompt_eval_count,
            Some(20)
        );
        assert!(requests.recv().unwrap().starts_with("GET /api/tags "));
        assert!(requests.recv().unwrap().starts_with("POST /api/generate "));
    }

    #[tokio::test]
    async fn injected_provider_streams_without_ollama_coupling() {
        let app = OpticCode::with_provider(
            Arc::new(FixtureProvider {
                invalid_sequence: false,
                health_delay: Duration::ZERO,
            }),
            "fixture-coder",
        )
        .unwrap();
        let (events, mut receiver) = assistant_event_channel(16).unwrap();
        let report = app
            .ask_with_protocol(
                ask_options("Locate Helpers#ping().", ContextMode::Legacy),
                AssistantProtocolSession {
                    request_id: "provider-fixture-1".to_string(),
                    events,
                    cancellation: CancellationToken::new(),
                },
            )
            .await
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = receiver.recv().await {
            observed.push(event);
        }

        assert!(report.success);
        assert_eq!(report.generation.endpoint, "fixture://local");
        assert_eq!(report.runs[0].response.as_deref(), Some("fixture response"));
        assert_eq!(
            observed.iter().filter(|event| event.is_terminal()).count(),
            1
        );
        let reconstructed = observed
            .iter()
            .filter_map(|event| event.output_delta())
            .collect::<String>();
        assert_eq!(reconstructed, "fixture response");
        let summary = match &observed.last().unwrap().payload {
            AssistantProtocolEventPayload::Completed {
                summary: Some(summary),
                ..
            } => summary,
            terminal => panic!("unexpected terminal event: {terminal:?}"),
        };
        assert_eq!(summary.used_context_mode, Some(ContextMode::Legacy));
        assert_eq!(summary.runs.len(), 1);
        assert!(summary.runs[0].generated);
        assert!(!summary.context_files.is_empty());
    }

    #[tokio::test]
    async fn invalid_provider_sequence_becomes_a_structured_assistant_failure() {
        let app = OpticCode::with_provider(
            Arc::new(FixtureProvider {
                invalid_sequence: true,
                health_delay: Duration::ZERO,
            }),
            "fixture-coder",
        )
        .unwrap();
        let (events, mut receiver) = assistant_event_channel(16).unwrap();
        let report = app
            .ask_with_protocol(
                ask_options("Locate Helpers#ping().", ContextMode::Legacy),
                AssistantProtocolSession {
                    request_id: "provider-fixture-bad-1".to_string(),
                    events,
                    cancellation: CancellationToken::new(),
                },
            )
            .await
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = receiver.recv().await {
            observed.push(event);
        }

        assert!(!report.success);
        assert_eq!(report.errors[0].code, "generation_failed");
        assert!(report.errors[0].message.contains("sequence mismatch"));
        assert!(matches!(
            observed.last().unwrap().payload,
            AssistantProtocolEventPayload::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_slow_provider_preflight() {
        let app = OpticCode::with_provider(
            Arc::new(FixtureProvider {
                invalid_sequence: false,
                health_delay: Duration::from_secs(5),
            }),
            "fixture-coder",
        )
        .unwrap();
        let (events, mut receiver) = assistant_event_channel(16).unwrap();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            app.ask_with_protocol(
                ask_options("Locate Helpers#ping().", ContextMode::Legacy),
                AssistantProtocolSession {
                    request_id: "cancel-preflight-1".to_string(),
                    events,
                    cancellation: task_cancellation,
                },
            )
            .await
            .unwrap()
        });

        assert!(matches!(
            receiver.recv().await.unwrap().payload,
            AssistantProtocolEventPayload::Started { .. }
        ));
        assert!(matches!(
            receiver.recv().await.unwrap().payload,
            AssistantProtocolEventPayload::ContextPrepared { .. }
        ));
        cancellation.cancel();
        let report = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        let terminal = receiver.recv().await.unwrap();

        assert!(!report.success);
        assert_eq!(report.errors[0].code, "request_cancelled");
        assert!(matches!(
            terminal.payload,
            AssistantProtocolEventPayload::Cancelled { .. }
        ));
        assert!(receiver.recv().await.is_none());
    }

    struct FixtureProvider {
        invalid_sequence: bool,
        health_delay: Duration,
    }

    #[async_trait]
    impl LlmProvider for FixtureProvider {
        fn id(&self) -> ProviderId {
            ProviderId::Ollama
        }

        fn endpoint(&self) -> &str {
            "fixture://local"
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
            if !self.health_delay.is_zero() {
                tokio::time::sleep(self.health_delay).await;
            }
            let model_available = request.model.as_ref().map(|_| true);
            Ok(HealthReport {
                schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
                provider: self.id(),
                endpoint: self.endpoint().to_string(),
                status: HealthStatus::Healthy,
                reachable: true,
                latency_ms: 0,
                model_count: 1,
                requested_model: request.model,
                model_available,
            })
        }

        async fn list_models(&self) -> std::result::Result<Vec<ModelInfo>, ProviderError> {
            Ok(Vec::new())
        }

        async fn generate(
            &self,
            request: GenerationRequest,
            _cancellation: CancellationToken,
        ) -> std::result::Result<GenerationResult, ProviderError> {
            Ok(fixture_generation_result(&request))
        }

        async fn stream(
            &self,
            request: GenerationRequest,
            events: EventSink,
            _cancellation: CancellationToken,
        ) -> std::result::Result<GenerationResult, ProviderError> {
            events
                .send(LlmProtocolEvent::new(
                    &request.request_id,
                    0,
                    LlmProtocolEventPayload::Started {
                        provider: self.id(),
                        model: request.model.clone(),
                    },
                ))
                .await
                .unwrap();
            events
                .send(LlmProtocolEvent::new(
                    &request.request_id,
                    if self.invalid_sequence { 2 } else { 1 },
                    LlmProtocolEventPayload::Delta {
                        text: "fixture response".to_string(),
                    },
                ))
                .await
                .unwrap();
            let result = fixture_generation_result(&request);
            if !self.invalid_sequence {
                events
                    .send(LlmProtocolEvent::new(
                        &request.request_id,
                        2,
                        LlmProtocolEventPayload::Completed {
                            result: result.clone(),
                        },
                    ))
                    .await
                    .unwrap();
            }
            Ok(result)
        }
    }

    fn fixture_generation_result(request: &GenerationRequest) -> GenerationResult {
        GenerationResult {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            provider: ProviderId::Ollama,
            model: request.model.clone(),
            output: "fixture response".to_string(),
            finish_reason: FinishReason::Stop,
            prompt_chars: request.prompt.chars().count(),
            usage: GenerationUsage {
                prompt_tokens: Some(10),
                generated_tokens: Some(2),
            },
            timings: GenerationTimings {
                client_ms: 1,
                provider_total_ms: Some(1),
                load_ms: Some(0),
                prompt_eval_ms: Some(1),
                generation_ms: Some(1),
            },
        }
    }

    #[test]
    fn provider_request_ids_stay_bounded_and_context_specific() {
        let assistant_id = "x".repeat(crate::protocol::MAX_ASSISTANT_REQUEST_ID_BYTES);
        let legacy = super::provider_request_id(&assistant_id, ContextMode::Legacy);
        let symbol = super::provider_request_id(&assistant_id, ContextMode::Symbol);

        assert!(legacy.len() <= opticcode_llm::MAX_REQUEST_ID_BYTES);
        assert!(opticcode_llm::validate_request_id(&legacy, ProviderId::Ollama).is_ok());
        assert_ne!(legacy, symbol);
        assert_eq!(
            super::provider_request_id("request-1", ContextMode::Legacy),
            "request-1:legacy"
        );
    }

    #[tokio::test]
    async fn plan_symbol_generates_when_analysis_is_complete() {
        let (app, _requests) = app(vec![TAGS, GENERATE]);
        let report = app
            .plan_with_report(PlanOptions {
                workspace: fixture(),
                goal: "Find plugin.yml main and commands.".to_string(),
                profile: None,
                include_memory: false,
                include_rag: false,
                rag_index: PathBuf::from("unused"),
                rag_limit: 4,
                brief: true,
                max_tokens: Some(32),
                temperature: Some(0.0),
                seed: Some(7),
                context_mode: ContextMode::Symbol,
                fallback_policy: ContextFallbackPolicy::Refuse,
                compare_generate: false,
                verify_model: true,
            })
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.used_context_mode, Some(ContextMode::Symbol));
        assert!(report.runs[0].generated);
    }

    #[tokio::test]
    async fn compare_does_not_contact_ollama_without_explicit_generation() {
        let app = OpticCode::try_new("http://127.0.0.1:9", "qwen2.5-coder:14b").unwrap();
        let report = app
            .ask_with_report(ask_options(
                "Locate dev.opticcode.util.Helpers#ping().",
                ContextMode::Compare,
            ))
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.runs.len(), 2);
        assert!(report.runs.iter().all(|run| !run.generated));
        assert!(!report.double_generation_authorized);
    }

    #[tokio::test]
    async fn ollama_preflight_error_codes_remain_backward_compatible() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let app = OpticCode::try_new(format!("http://{address}"), "qwen2.5-coder:14b")
            .unwrap()
            .with_http_timeout(Duration::from_millis(200))
            .unwrap();

        let report = app
            .ask_with_report(ask_options("Locate Helpers#ping().", ContextMode::Legacy))
            .await
            .unwrap();

        assert!(!report.success);
        assert_eq!(report.errors[0].code, "ollama_unavailable");
        assert_eq!(report.errors[0].stage, "ollama_preflight");
    }

    #[tokio::test]
    async fn compare_generates_two_distinct_runs_only_when_authorized() {
        let (app, requests) = app(vec![TAGS, GENERATE, GENERATE]);
        let mut options = ask_options(
            "Locate dev.opticcode.util.Helpers#ping().",
            ContextMode::Compare,
        );
        options.compare_generate = true;

        let report = app.ask_with_report(options).await.unwrap();

        assert!(report.success);
        assert!(report.double_generation_authorized);
        assert_eq!(report.runs.iter().filter(|run| run.generated).count(), 2);
        assert!(requests.recv().unwrap().starts_with("GET /api/tags "));
        assert!(requests.recv().unwrap().starts_with("POST /api/generate "));
        assert!(requests.recv().unwrap().starts_with("POST /api/generate "));
    }

    #[tokio::test]
    async fn symbol_fallback_is_explicit_in_the_generated_report() {
        let (app, _requests) = app(vec![TAGS, GENERATE]);
        let report = app
            .ask_with_report(ask_options(
                "Inspect dev.opticcode.util.Helpers#create(String).",
                ContextMode::Symbol,
            ))
            .await
            .unwrap();

        assert!(report.success);
        assert_eq!(report.used_context_mode, Some(ContextMode::Legacy));
        assert!(report.context.fallback.as_ref().unwrap().applied);
        assert_eq!(report.runs[0].context_mode, ContextMode::Legacy);
    }

    #[tokio::test]
    async fn strict_ambiguous_symbol_context_fails_before_network_access() {
        let app = OpticCode::try_new("http://127.0.0.1:9", "qwen2.5-coder:14b").unwrap();
        let mut options = ask_options("Inspect Duplicate.", ContextMode::Symbol);
        options.fallback_policy = ContextFallbackPolicy::Refuse;

        let report = app.ask_with_report(options).await.unwrap();

        assert!(!report.success);
        assert_eq!(report.used_context_mode, None);
        assert_eq!(report.errors[0].code, "context_rejected");
        assert!(report.runs.iter().all(|run| !run.generated));
    }

    fn spawn_mock(responses: Vec<&'static str>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                sender.send(read_http_request(&mut stream)).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}"), receiver)
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let read = stream.read(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let header_end = header_end + 4;
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length: ")
                        .or_else(|| line.strip_prefix("Content-Length: "))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= header_end + content_length {
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
