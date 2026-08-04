use std::env;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    CancellationToken, EventSink, FinishReason, GenerationOutputFormat, GenerationRequest,
    GenerationResult, GenerationTimings, GenerationUsage, HealthReport, HealthRequest,
    HealthStatus, LlmProtocolEvent, LlmProtocolEventPayload, LlmProvider, ModelInfo,
    ProviderCapabilities, ProviderError, ProviderErrorKind, ProviderId,
    LLM_PROTOCOL_SCHEMA_VERSION, MAX_GENERATED_OUTPUT_BYTES,
};

pub const DEFAULT_OLLAMA_HTTP_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_OLLAMA_HTTP_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const MAX_STREAM_LINE_BYTES: usize = 1024 * 1024;
const MAX_NON_STREAM_BODY_BYTES: usize = MAX_GENERATED_OUTPUT_BYTES + MAX_STREAM_LINE_BYTES;
const MAX_HTTP_ERROR_DETAIL_BYTES: usize = 4 * 1024;
const EVENT_DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct OllamaProvider {
    base_url: String,
    timeout: Duration,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    provider: OllamaProvider,
    keep_alive: Option<String>,
    timeout: Duration,
}

#[derive(Debug, Serialize)]
struct OllamaGenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaWireOptions>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct OllamaWireOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct GenerateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct OllamaModelDetails {
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub parameter_size: String,
    #[serde(default)]
    pub quantization_level: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OllamaModelInfo {
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub details: OllamaModelDetails,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OllamaGenerateResponse {
    #[serde(default)]
    response: String,
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    total_duration: Option<u64>,
    load_duration: Option<u64>,
    prompt_eval_count: Option<u64>,
    prompt_eval_duration: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct GenerateResponse {
    pub response: String,
    pub done: bool,
    pub total_duration: Option<u64>,
    pub load_duration: Option<u64>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_duration: Option<u64>,
    pub eval_count: Option<u64>,
    pub eval_duration: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct GenerateMetrics {
    pub client_duration: Duration,
    pub prompt_chars: usize,
    pub ollama_total_duration: Option<Duration>,
    pub ollama_load_duration: Option<Duration>,
    pub keep_alive: Option<String>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_duration: Option<Duration>,
    pub eval_count: Option<u64>,
    pub eval_duration: Option<Duration>,
}

#[derive(Debug)]
pub struct TimedGenerateResponse {
    pub response: String,
    pub metrics: GenerateMetrics,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            timeout: DEFAULT_OLLAMA_HTTP_TIMEOUT,
            http: reqwest::Client::new(),
        }
    }

    pub fn try_new(base_url: impl AsRef<str>) -> std::result::Result<Self, ProviderError> {
        let base_url = validate_local_ollama_url(base_url.as_ref()).map_err(|error| {
            ProviderError::invalid_configuration(
                ProviderId::Ollama,
                "provider_setup",
                format!("{error:#}"),
            )
        })?;
        Ok(Self::new(base_url))
    }

    pub fn with_default_timeout(
        mut self,
        timeout: Duration,
    ) -> std::result::Result<Self, ProviderError> {
        validate_timeout_duration(timeout)?;
        self.timeout = timeout;
        Ok(self)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    async fn list_models_raw(
        &self,
        timeout: Duration,
    ) -> std::result::Result<Vec<OllamaModelInfo>, ProviderError> {
        validate_provider_url(&self.base_url, "model_inventory")?;
        let started_at = Instant::now();
        let response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .timeout(timeout)
            .send()
            .await
            .map_err(|error| map_reqwest_error(error, "model_inventory"))?;
        let response = checked_status(response, "model_inventory").await?;
        let parsed = response
            .json::<OllamaTagsResponse>()
            .await
            .map_err(|error| map_reqwest_error(error, "model_inventory_parse"))?;
        let _ = started_at;
        Ok(parsed.models)
    }

    async fn send_generate_request(
        &self,
        request: &GenerationRequest,
        stream: bool,
        cancellation: &CancellationToken,
    ) -> std::result::Result<reqwest::Response, ProviderError> {
        validate_provider_url(&self.base_url, "generation_request")?;
        request.validate(ProviderId::Ollama)?;
        let wire_options = OllamaWireOptions {
            num_predict: request.options.max_output_tokens,
            temperature: request.options.temperature,
            seed: request.options.seed,
        };
        let wire = OllamaGenerateRequest {
            model: &request.model,
            prompt: &request.prompt,
            stream,
            format: match request.options.output_format {
                GenerationOutputFormat::Text => None,
                GenerationOutputFormat::Json => Some(
                    request
                        .options
                        .output_schema
                        .clone()
                        .unwrap_or_else(|| serde_json::Value::String("json".to_string())),
                ),
            },
            keep_alive: request.options.keep_alive.as_deref(),
            options: wire_options.has_values().then_some(wire_options),
        };
        let send = self
            .http
            .post(format!("{}/api/generate", self.base_url))
            .json(&wire)
            .timeout(Duration::from_millis(request.options.timeout_ms))
            .send();
        tokio::pin!(send);
        let response = tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(ProviderError::cancelled(ProviderId::Ollama, "generation_send"));
            }
            response = &mut send => {
                response.map_err(|error| map_reqwest_error(error, "generation_send"))?
            }
        };
        checked_status(response, "generation_status").await
    }

    async fn generate_inner(
        &self,
        request: &GenerationRequest,
        cancellation: &CancellationToken,
    ) -> std::result::Result<GenerationResult, ProviderError> {
        let started_at = Instant::now();
        let response = self
            .send_generate_request(request, false, cancellation)
            .await?;
        let body = read_bounded_response_body(response, cancellation).await?;
        let response =
            serde_json::from_slice::<OllamaGenerateResponse>(&body).map_err(|error| {
                ProviderError::new(
                    ProviderId::Ollama,
                    ProviderErrorKind::Protocol,
                    "generation_parse",
                    false,
                    format!("failed to parse Ollama generation response: {error}"),
                )
            })?;
        if !response.done {
            return Err(ProviderError::new(
                ProviderId::Ollama,
                ProviderErrorKind::Protocol,
                "generation_parse",
                false,
                "non-streaming Ollama response did not contain done=true",
            ));
        }
        ensure_output_capacity(0, response.response.len(), "generation_parse")?;
        Ok(generation_result(request, response, started_at.elapsed()))
    }

    async fn stream_inner(
        &self,
        request: &GenerationRequest,
        events: &EventSink,
        cancellation: &CancellationToken,
        sequence: &mut u64,
    ) -> std::result::Result<GenerationResult, ProviderError> {
        let started_at = Instant::now();
        let mut response = self
            .send_generate_request(request, true, cancellation)
            .await?;
        let mut buffer = Vec::new();
        let mut output = String::new();
        let mut final_chunk = None;

        while final_chunk.is_none() {
            let chunk = tokio::select! {
                _ = cancellation.cancelled() => {
                    return Err(ProviderError::cancelled(ProviderId::Ollama, "generation_stream"));
                }
                chunk = response.chunk() => {
                    chunk.map_err(|error| map_reqwest_error(error, "generation_stream"))?
                }
            };
            let Some(chunk) = chunk else {
                break;
            };
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_STREAM_LINE_BYTES && !buffer.contains(&b'\n') {
                return Err(stream_line_too_large());
            }

            let mut consumed = 0usize;
            while let Some(relative_end) = buffer[consumed..].iter().position(|byte| *byte == b'\n')
            {
                let end = consumed + relative_end;
                let line = &buffer[consumed..end];
                consumed = end + 1;
                if line.len() > MAX_STREAM_LINE_BYTES {
                    return Err(stream_line_too_large());
                }
                if let Some(done) =
                    process_stream_line(line, request, events, cancellation, sequence, &mut output)
                        .await?
                {
                    final_chunk = Some(done);
                    break;
                }
            }
            if consumed > 0 {
                buffer.drain(..consumed);
            }
        }

        if final_chunk.is_none() && !buffer.iter().all(u8::is_ascii_whitespace) {
            final_chunk = process_stream_line(
                &buffer,
                request,
                events,
                cancellation,
                sequence,
                &mut output,
            )
            .await?;
        }
        let Some(mut done) = final_chunk else {
            return Err(ProviderError::new(
                ProviderId::Ollama,
                ProviderErrorKind::Protocol,
                "generation_stream",
                true,
                "Ollama stream ended without a terminal done=true object",
            ));
        };
        done.response = output;
        Ok(generation_result(request, done, started_at.elapsed()))
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn endpoint(&self) -> &str {
        &self.base_url
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
        crate::validate_schema(request.schema_version, ProviderId::Ollama, "health")?;
        crate::validate_timeout(request.timeout_ms, ProviderId::Ollama, "health")?;
        if let Some(model) = request.model.as_deref() {
            crate::validate_model_name(model, ProviderId::Ollama)?;
        }
        let started_at = Instant::now();
        let models = self
            .list_models_raw(Duration::from_millis(request.timeout_ms))
            .await?;
        let model_available = request.model.as_deref().map(|model| {
            models
                .iter()
                .any(|candidate| model_matches(candidate, model))
        });
        Ok(HealthReport {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            provider: ProviderId::Ollama,
            endpoint: self.base_url.clone(),
            status: if model_available == Some(false) {
                HealthStatus::ModelUnavailable
            } else {
                HealthStatus::Healthy
            },
            reachable: true,
            latency_ms: duration_ms(started_at.elapsed()),
            model_count: models.len(),
            requested_model: request.model,
            model_available,
        })
    }

    async fn list_models(&self) -> std::result::Result<Vec<ModelInfo>, ProviderError> {
        Ok(self
            .list_models_raw(self.timeout)
            .await?
            .into_iter()
            .map(model_info)
            .collect())
    }

    async fn generate(
        &self,
        request: GenerationRequest,
        cancellation: CancellationToken,
    ) -> std::result::Result<GenerationResult, ProviderError> {
        request.validate(ProviderId::Ollama)?;
        if cancellation.is_cancelled() {
            return Err(ProviderError::cancelled(
                ProviderId::Ollama,
                "generation_start",
            ));
        }
        self.generate_inner(&request, &cancellation).await
    }

    async fn stream(
        &self,
        request: GenerationRequest,
        events: EventSink,
        cancellation: CancellationToken,
    ) -> std::result::Result<GenerationResult, ProviderError> {
        request.validate(ProviderId::Ollama)?;
        let mut sequence = 0u64;
        if cancellation.is_cancelled() {
            let error = ProviderError::cancelled(ProviderId::Ollama, "generation_start");
            send_terminal_event(
                &events,
                LlmProtocolEvent::new(
                    &request.request_id,
                    sequence,
                    LlmProtocolEventPayload::Cancelled {
                        reason: error.message.clone(),
                    },
                ),
            )
            .await?;
            return Err(error);
        }
        if let Err(error) = emit_event(
            &events,
            &request.request_id,
            &mut sequence,
            LlmProtocolEventPayload::Started {
                provider: ProviderId::Ollama,
                model: request.model.clone(),
            },
            &cancellation,
        )
        .await
        {
            if error.kind == ProviderErrorKind::Cancelled {
                send_terminal_event(
                    &events,
                    LlmProtocolEvent::new(
                        &request.request_id,
                        sequence,
                        LlmProtocolEventPayload::Cancelled {
                            reason: error.message.clone(),
                        },
                    ),
                )
                .await?;
            }
            return Err(error);
        }
        let result = self
            .stream_inner(&request, &events, &cancellation, &mut sequence)
            .await;
        match result {
            Ok(result) => {
                send_terminal_event(
                    &events,
                    LlmProtocolEvent::new(
                        &request.request_id,
                        sequence,
                        LlmProtocolEventPayload::Completed {
                            result: result.clone(),
                        },
                    ),
                )
                .await?;
                Ok(result)
            }
            Err(error) => {
                let payload = if error.kind == ProviderErrorKind::Cancelled {
                    LlmProtocolEventPayload::Cancelled {
                        reason: error.message.clone(),
                    }
                } else {
                    LlmProtocolEventPayload::Failed {
                        error: error.clone(),
                    }
                };
                let terminal = LlmProtocolEvent::new(&request.request_id, sequence, payload);
                if let Err(delivery_error) = send_terminal_event(&events, terminal).await {
                    if error.kind != ProviderErrorKind::EventSinkClosed {
                        return Err(delivery_error);
                    }
                }
                Err(error)
            }
        }
    }
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            provider: OllamaProvider::new(base_url),
            keep_alive: default_ollama_keep_alive(),
            timeout: DEFAULT_OLLAMA_HTTP_TIMEOUT,
        }
    }

    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self> {
        let provider = OllamaProvider::try_new(base_url).map_err(anyhow::Error::new)?;
        Ok(Self {
            provider,
            keep_alive: default_ollama_keep_alive(),
            timeout: DEFAULT_OLLAMA_HTTP_TIMEOUT,
        })
    }

    pub fn with_keep_alive(mut self, keep_alive: Option<String>) -> Self {
        self.keep_alive = keep_alive;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self> {
        validate_timeout_duration(timeout).map_err(anyhow::Error::new)?;
        self.timeout = timeout;
        Ok(self)
    }

    pub fn base_url(&self) -> &str {
        self.provider.base_url()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub async fn list_models(&self) -> Result<Vec<OllamaModelInfo>> {
        self.provider
            .list_models_raw(self.timeout)
            .await
            .map_err(anyhow::Error::new)
    }

    pub async fn model_available(&self, model: &str) -> Result<bool> {
        if model.trim().is_empty() {
            bail!("Ollama model name must not be empty");
        }
        let models = self.list_models().await?;
        Ok(models
            .iter()
            .any(|candidate| model_matches(candidate, model)))
    }

    pub async fn generate(&self, model: &str, prompt: &str) -> Result<GenerateResponse> {
        Ok(self.generate_timed(model, prompt).await?.into_response())
    }

    pub async fn generate_timed(&self, model: &str, prompt: &str) -> Result<TimedGenerateResponse> {
        self.generate_timed_with_options(model, prompt, GenerateOptions::default())
            .await
    }

    pub async fn generate_timed_with_options(
        &self,
        model: &str,
        prompt: &str,
        options: GenerateOptions,
    ) -> Result<TimedGenerateResponse> {
        let mut request = GenerationRequest::new("legacy-generate", model, prompt);
        request.options.max_output_tokens = options.num_predict;
        request.options.temperature = options.temperature;
        request.options.seed = options.seed;
        request.options.keep_alive = self.keep_alive.clone();
        request.options.timeout_ms = duration_ms_ceil(self.timeout);
        let result = self
            .provider
            .generate(request, CancellationToken::new())
            .await
            .map_err(anyhow::Error::new)?;
        Ok(TimedGenerateResponse {
            response: result.output,
            metrics: GenerateMetrics {
                client_duration: Duration::from_millis(result.timings.client_ms),
                prompt_chars: result.prompt_chars,
                ollama_total_duration: result.timings.provider_total_ms.map(Duration::from_millis),
                ollama_load_duration: result.timings.load_ms.map(Duration::from_millis),
                keep_alive: self.keep_alive.clone(),
                prompt_eval_count: result.usage.prompt_tokens,
                prompt_eval_duration: result.timings.prompt_eval_ms.map(Duration::from_millis),
                eval_count: result.usage.generated_tokens,
                eval_duration: result.timings.generation_ms.map(Duration::from_millis),
            },
        })
    }
}

impl OllamaWireOptions {
    fn has_values(self) -> bool {
        self.num_predict.is_some() || self.temperature.is_some() || self.seed.is_some()
    }
}

impl TimedGenerateResponse {
    fn into_response(self) -> GenerateResponse {
        GenerateResponse {
            response: self.response,
            done: true,
            total_duration: self.metrics.ollama_total_duration.map(duration_to_nanos),
            load_duration: self.metrics.ollama_load_duration.map(duration_to_nanos),
            prompt_eval_count: self.metrics.prompt_eval_count,
            prompt_eval_duration: self.metrics.prompt_eval_duration.map(duration_to_nanos),
            eval_count: self.metrics.eval_count,
            eval_duration: self.metrics.eval_duration.map(duration_to_nanos),
        }
    }
}

async fn process_stream_line(
    raw_line: &[u8],
    request: &GenerationRequest,
    events: &EventSink,
    cancellation: &CancellationToken,
    sequence: &mut u64,
    output: &mut String,
) -> std::result::Result<Option<OllamaGenerateResponse>, ProviderError> {
    let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
    if line.is_empty() {
        return Ok(None);
    }
    let chunk = serde_json::from_slice::<OllamaGenerateResponse>(line).map_err(|error| {
        ProviderError::new(
            ProviderId::Ollama,
            ProviderErrorKind::Protocol,
            "generation_stream_parse",
            false,
            format!("failed to parse Ollama NDJSON event: {error}"),
        )
    })?;
    if !chunk.response.is_empty() {
        ensure_output_capacity(
            output.len(),
            chunk.response.len(),
            "generation_stream_parse",
        )?;
        output.push_str(&chunk.response);
        emit_event(
            events,
            &request.request_id,
            sequence,
            LlmProtocolEventPayload::Delta {
                text: chunk.response.clone(),
            },
            cancellation,
        )
        .await?;
    }
    Ok(chunk.done.then_some(chunk))
}

async fn emit_event(
    events: &EventSink,
    request_id: &str,
    sequence: &mut u64,
    payload: LlmProtocolEventPayload,
    cancellation: &CancellationToken,
) -> std::result::Result<(), ProviderError> {
    let event = LlmProtocolEvent::new(request_id, *sequence, payload);
    let delivery = tokio::time::timeout(EVENT_DELIVERY_TIMEOUT, events.send(event));
    tokio::select! {
        _ = cancellation.cancelled() => {
            Err(ProviderError::cancelled(ProviderId::Ollama, "event_delivery"))
        }
        sent = delivery => {
            sent.map_err(|_| event_delivery_timeout("event_delivery"))?
                .map_err(|_| event_sink_closed("event_delivery"))?;
            *sequence = sequence.saturating_add(1);
            Ok(())
        }
    }
}

async fn send_terminal_event(
    events: &EventSink,
    event: LlmProtocolEvent,
) -> std::result::Result<(), ProviderError> {
    tokio::time::timeout(EVENT_DELIVERY_TIMEOUT, events.send(event))
        .await
        .map_err(|_| event_delivery_timeout("terminal_event"))?
        .map_err(|_| event_sink_closed("terminal_event"))
}

async fn read_bounded_response_body(
    mut response: reqwest::Response,
    cancellation: &CancellationToken,
) -> std::result::Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => {
                return Err(ProviderError::cancelled(ProviderId::Ollama, "generation_body"));
            }
            chunk = response.chunk() => {
                chunk.map_err(|error| map_reqwest_error(error, "generation_body"))?
            }
        };
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        let next_len = body.len().checked_add(chunk.len()).ok_or_else(|| {
            generation_output_too_large("generation_body", MAX_NON_STREAM_BODY_BYTES)
        })?;
        if next_len > MAX_NON_STREAM_BODY_BYTES {
            return Err(generation_output_too_large(
                "generation_body",
                MAX_NON_STREAM_BODY_BYTES,
            ));
        }
        body.extend_from_slice(&chunk);
    }
}

fn ensure_output_capacity(
    current_len: usize,
    additional_len: usize,
    stage: &str,
) -> std::result::Result<(), ProviderError> {
    if current_len
        .checked_add(additional_len)
        .is_none_or(|length| length > MAX_GENERATED_OUTPUT_BYTES)
    {
        return Err(generation_output_too_large(
            stage,
            MAX_GENERATED_OUTPUT_BYTES,
        ));
    }
    Ok(())
}

fn generation_result(
    request: &GenerationRequest,
    response: OllamaGenerateResponse,
    client_duration: Duration,
) -> GenerationResult {
    GenerationResult {
        schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
        request_id: request.request_id.clone(),
        provider: ProviderId::Ollama,
        model: request.model.clone(),
        output: response.response,
        finish_reason: finish_reason(response.done_reason.as_deref()),
        prompt_chars: request.prompt.chars().count(),
        usage: GenerationUsage {
            prompt_tokens: response.prompt_eval_count,
            generated_tokens: response.eval_count,
        },
        timings: GenerationTimings {
            client_ms: duration_ms(client_duration),
            provider_total_ms: response.total_duration.map(nanos_to_millis),
            load_ms: response.load_duration.map(nanos_to_millis),
            prompt_eval_ms: response.prompt_eval_duration.map(nanos_to_millis),
            generation_ms: response.eval_duration.map(nanos_to_millis),
        },
    }
}

fn model_info(model: OllamaModelInfo) -> ModelInfo {
    ModelInfo {
        schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
        provider: ProviderId::Ollama,
        name: model.name,
        alias: (!model.model.is_empty()).then_some(model.model),
        size_bytes: model.size,
        digest: (!model.digest.is_empty()).then_some(model.digest),
        family: (!model.details.family.is_empty()).then_some(model.details.family),
        parameter_size: (!model.details.parameter_size.is_empty())
            .then_some(model.details.parameter_size),
        quantization: (!model.details.quantization_level.is_empty())
            .then_some(model.details.quantization_level),
    }
}

fn model_matches(candidate: &OllamaModelInfo, model: &str) -> bool {
    candidate.name.eq_ignore_ascii_case(model)
        || candidate.model.eq_ignore_ascii_case(model)
        || strip_latest(&candidate.name).eq_ignore_ascii_case(strip_latest(model))
        || strip_latest(&candidate.model).eq_ignore_ascii_case(strip_latest(model))
}

fn finish_reason(value: Option<&str>) -> FinishReason {
    match value.unwrap_or("stop").to_ascii_lowercase().as_str() {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "unload" => FinishReason::Unloaded,
        _ => FinishReason::Unknown,
    }
}

async fn checked_status(
    response: reqwest::Response,
    stage: &str,
) -> std::result::Result<reqwest::Response, ProviderError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let detail = read_bounded_http_error_detail(response).await;
    Err(ProviderError::new(
        ProviderId::Ollama,
        ProviderErrorKind::HttpStatus,
        stage,
        status.is_server_error() || status.as_u16() == 429,
        match detail {
            Some(detail) => format!("local Ollama API returned HTTP {status}: {detail}"),
            None => format!("local Ollama API returned HTTP {status}"),
        },
    ))
}

async fn read_bounded_http_error_detail(mut response: reqwest::Response) -> Option<String> {
    let mut bytes = Vec::with_capacity(MAX_HTTP_ERROR_DETAIL_BYTES);
    while bytes.len() < MAX_HTTP_ERROR_DETAIL_BYTES {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return None,
        };
        let remaining = MAX_HTTP_ERROR_DETAIL_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    let detail = bounded_http_error_detail(&bytes);
    (!detail.is_empty()).then_some(detail)
}

fn bounded_http_error_detail(bytes: &[u8]) -> String {
    let bytes = &bytes[..bytes.len().min(MAX_HTTP_ERROR_DETAIL_BYTES)];
    let value = String::from_utf8_lossy(bytes);
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn map_reqwest_error(error: reqwest::Error, stage: &str) -> ProviderError {
    if error.is_timeout() {
        ProviderError::new(
            ProviderId::Ollama,
            ProviderErrorKind::Timeout,
            stage,
            true,
            "failed to contact local Ollama API: request exceeded its explicit timeout",
        )
    } else {
        ProviderError::new(
            ProviderId::Ollama,
            ProviderErrorKind::Unavailable,
            stage,
            true,
            format!("failed to contact local Ollama API: {error}"),
        )
    }
}

fn validate_provider_url(value: &str, stage: &str) -> std::result::Result<(), ProviderError> {
    validate_local_ollama_url(value)
        .map(|_| ())
        .map_err(|error| {
            ProviderError::invalid_configuration(ProviderId::Ollama, stage, format!("{error:#}"))
        })
}

fn validate_timeout_duration(timeout: Duration) -> std::result::Result<(), ProviderError> {
    if timeout.is_zero() || timeout > MAX_OLLAMA_HTTP_TIMEOUT {
        return Err(ProviderError::invalid_configuration(
            ProviderId::Ollama,
            "provider_setup",
            format!(
                "Ollama HTTP timeout must be between 1 ns and {} seconds",
                MAX_OLLAMA_HTTP_TIMEOUT.as_secs()
            ),
        ));
    }
    Ok(())
}

fn stream_line_too_large() -> ProviderError {
    ProviderError::new(
        ProviderId::Ollama,
        ProviderErrorKind::Protocol,
        "generation_stream_parse",
        false,
        format!(
            "Ollama NDJSON event exceeds the {} byte limit",
            MAX_STREAM_LINE_BYTES
        ),
    )
}

fn event_sink_closed(stage: &str) -> ProviderError {
    ProviderError::new(
        ProviderId::Ollama,
        ProviderErrorKind::EventSinkClosed,
        stage,
        false,
        "LLM protocol event receiver was closed",
    )
}

fn event_delivery_timeout(stage: &str) -> ProviderError {
    ProviderError::new(
        ProviderId::Ollama,
        ProviderErrorKind::Timeout,
        stage,
        false,
        "LLM protocol event delivery timed out",
    )
}

fn generation_output_too_large(stage: &str, limit: usize) -> ProviderError {
    ProviderError::new(
        ProviderId::Ollama,
        ProviderErrorKind::Protocol,
        stage,
        false,
        format!("Ollama generation response exceeds the {limit} byte limit"),
    )
}

fn nanos_to_millis(value: u64) -> u64 {
    Duration::from_nanos(value)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn duration_ms(value: Duration) -> u64 {
    value.as_millis().min(u128::from(u64::MAX)) as u64
}

fn duration_ms_ceil(value: Duration) -> u64 {
    let nanos = value.as_nanos();
    nanos
        .saturating_add(999_999)
        .checked_div(1_000_000)
        .unwrap_or(u128::from(u64::MAX))
        .min(u128::from(u64::MAX)) as u64
}

fn duration_to_nanos(value: Duration) -> u64 {
    value.as_nanos().min(u128::from(u64::MAX)) as u64
}

pub fn default_ollama_keep_alive() -> Option<String> {
    match env::var("OPTICCODE_OLLAMA_KEEP_ALIVE") {
        Ok(value) => parse_keep_alive(&value),
        Err(_) => Some("15m".to_string()),
    }
}

pub fn parse_keep_alive(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn validate_local_ollama_url(value: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(value).context("invalid Ollama URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("Ollama URL must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Ollama URL must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("Ollama URL must not contain a query or fragment");
    }
    if !matches!(parsed.path(), "" | "/") {
        bail!("Ollama URL must not contain an API path");
    }
    let host = parsed.host_str().context("Ollama URL has no host")?;
    let ip_host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    let local = host.eq_ignore_ascii_case("localhost")
        || ip_host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !local {
        bail!("refusing non-local Ollama URL; only localhost and loopback IPs are allowed");
    }
    let mut normalized = parsed;
    normalized.set_path("");
    Ok(normalized.as_str().trim_end_matches('/').to_string())
}

fn strip_latest(value: &str) -> &str {
    value.strip_suffix(":latest").unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    use crate::{
        event_channel, CancellationToken, GenerationOutputFormat, GenerationRequest, HealthRequest,
        HealthStatus, LlmProtocolEventPayload, LlmProvider, ProviderErrorKind,
    };

    use super::{
        bounded_http_error_detail, ensure_output_capacity, parse_keep_alive,
        validate_local_ollama_url, GenerateOptions, OllamaClient, OllamaProvider,
    };

    #[test]
    fn parses_keep_alive_values() {
        assert_eq!(parse_keep_alive("15m").as_deref(), Some("15m"));
        assert_eq!(parse_keep_alive("0").as_deref(), Some("0"));
        assert_eq!(parse_keep_alive(" none "), None);
        assert_eq!(parse_keep_alive(""), None);
    }

    #[test]
    fn bounds_and_sanitizes_local_http_error_details() {
        let detail = bounded_http_error_detail(b"grammar\r\nfailed\0");
        assert_eq!(detail, "grammar  failed");
        assert!(bounded_http_error_detail(&[b'x'; 8 * 1024]).len() <= 4 * 1024);
    }

    #[test]
    fn accepts_only_local_ollama_urls() {
        assert_eq!(
            validate_local_ollama_url("http://localhost:11434/").unwrap(),
            "http://localhost:11434"
        );
        assert!(validate_local_ollama_url("http://127.0.0.1:11434").is_ok());
        assert!(validate_local_ollama_url("http://[::1]:11434").is_ok());
        assert!(validate_local_ollama_url("https://example.com").is_err());
        assert!(validate_local_ollama_url("http://localhost:11434/api/generate").is_err());
        assert!(validate_local_ollama_url("http://user:secret@localhost:11434").is_err());
    }

    #[test]
    fn rejects_unbounded_http_timeouts() {
        assert!(OllamaClient::new("http://localhost:11434")
            .with_timeout(Duration::ZERO)
            .is_err());
        assert!(OllamaClient::new("http://localhost:11434")
            .with_timeout(Duration::from_secs(3_601))
            .is_err());
    }

    #[test]
    fn rejects_unbounded_cumulative_generation_output() {
        assert!(ensure_output_capacity(0, crate::MAX_GENERATED_OUTPUT_BYTES, "test").is_ok());
        let error =
            ensure_output_capacity(crate::MAX_GENERATED_OUTPUT_BYTES, 1, "test").unwrap_err();
        assert_eq!(error.kind, ProviderErrorKind::Protocol);
    }

    #[tokio::test]
    async fn reports_an_absent_local_ollama_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = OllamaClient::try_new(format!("http://{address}"))
            .unwrap()
            .with_timeout(Duration::from_millis(200))
            .unwrap();

        let error = client.list_models().await.unwrap_err();

        assert!(format!("{error:#}").contains("failed to contact local Ollama API"));
    }

    #[tokio::test]
    async fn detects_a_missing_model_from_the_local_inventory() {
        let (url, request) = spawn_mock(
            r#"{"models":[{"name":"other-coder:7b","model":"other-coder:7b","size":42}]}"#,
            Duration::ZERO,
        );
        let client = OllamaClient::try_new(url).unwrap();

        assert!(!client.model_available("qwen2.5-coder:14b").await.unwrap());
        assert!(request.recv().unwrap().starts_with("GET /api/tags "));
    }

    #[tokio::test]
    async fn sends_reproducible_generation_options() {
        let (url, request) = spawn_mock(
            r#"{"response":"ok","done":true,"total_duration":10,"load_duration":2,"prompt_eval_count":12,"prompt_eval_duration":3,"eval_count":4,"eval_duration":5}"#,
            Duration::ZERO,
        );
        let client = OllamaClient::try_new(url).unwrap();
        let response = client
            .generate_timed_with_options(
                "qwen2.5-coder:14b",
                "local source",
                GenerateOptions {
                    num_predict: Some(64),
                    temperature: Some(0.0),
                    seed: Some(42),
                },
            )
            .await
            .unwrap();
        let request = request.recv().unwrap();

        assert_eq!(response.response, "ok");
        assert_eq!(response.metrics.prompt_eval_count, Some(12));
        assert!(request.contains(r#""stream":false"#));
        assert!(request.contains(r#""num_predict":64"#));
        assert!(request.contains(r#""temperature":0.0"#));
        assert!(request.contains(r#""seed":42"#));
    }

    #[tokio::test]
    async fn sends_native_json_format_for_structured_generation() {
        let (url, request) = spawn_mock(
            r#"{"response":"{\"ok\":true}","done":true}"#,
            Duration::ZERO,
        );
        let provider = OllamaProvider::try_new(url).unwrap();
        let mut generation = GenerationRequest::new("structured-1", "qwen", "return JSON");
        generation.options.output_format = GenerationOutputFormat::Json;

        let result = provider
            .generate(generation, CancellationToken::new())
            .await
            .unwrap();
        let request = request.recv().unwrap();

        assert_eq!(result.output, r#"{"ok":true}"#);
        assert!(request.contains(r#""format":"json""#));
    }

    #[tokio::test]
    async fn sends_native_schema_for_constrained_generation() {
        let (url, request) = spawn_mock(
            r#"{"response":"{\"answer\":true}","done":true}"#,
            Duration::ZERO,
        );
        let provider = OllamaProvider::try_new(url).unwrap();
        let mut generation = GenerationRequest::new("schema-1", "qwen", "return JSON");
        generation.options.output_format = GenerationOutputFormat::Json;
        generation.options.output_schema = Some(serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "boolean"}},
            "required": ["answer"],
            "additionalProperties": false
        }));

        provider
            .generate(generation, CancellationToken::new())
            .await
            .unwrap();
        let request = request.recv().unwrap();

        assert!(request.contains(r#""format":{"additionalProperties":false"#));
        assert!(request.contains(r#""answer":{"type":"boolean"}"#));
    }

    #[tokio::test]
    async fn bounds_http_error_details_while_reading_the_response() {
        let body = format!("grammar\r\nfailed:{}", "x".repeat(8 * 1024));
        let (url, _request) = spawn_status_mock("400 Bad Request", body);
        let provider = OllamaProvider::try_new(url).unwrap();
        let request = GenerationRequest::new("error-1", "qwen", "return JSON");

        let error = provider
            .generate(request, CancellationToken::new())
            .await
            .unwrap_err();

        assert_eq!(error.kind, ProviderErrorKind::HttpStatus);
        assert!(error.message.contains("grammar  failed"));
        assert!(error.message.len() < 5 * 1024);
        assert!(!error.message.contains('\n'));
    }

    #[tokio::test]
    async fn enforces_the_explicit_http_timeout() {
        let (url, _request) = spawn_mock(
            r#"{"response":"late","done":true,"total_duration":10,"load_duration":2,"prompt_eval_count":1,"prompt_eval_duration":3,"eval_count":1,"eval_duration":5}"#,
            Duration::from_millis(250),
        );
        let client = OllamaClient::try_new(url)
            .unwrap()
            .with_timeout(Duration::from_millis(50))
            .unwrap();

        let error = client.generate_timed("qwen", "prompt").await.unwrap_err();

        assert!(format!("{error:#}").contains("explicit timeout"));
    }

    #[tokio::test]
    async fn streams_ordered_events_and_reconstructs_output() {
        let body = concat!(
            "{\"response\":\"hel\",\"done\":false}\n",
            "{\"response\":\"lo\",\"done\":false}\n",
            "{\"response\":\"\",\"done\":true,\"done_reason\":\"stop\",",
            "\"total_duration\":1000000,\"load_duration\":1000,",
            "\"prompt_eval_count\":3,\"prompt_eval_duration\":2000,",
            "\"eval_count\":2,\"eval_duration\":3000}\n"
        );
        let (url, _request) = spawn_stream_mock(body, Duration::ZERO);
        let provider = OllamaProvider::try_new(url).unwrap();
        let (events, mut receiver) = event_channel(16).unwrap();
        let request = GenerationRequest::new("stream-1", "qwen", "prompt");

        let result = provider
            .stream(request, events, CancellationToken::new())
            .await
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = receiver.recv().await {
            observed.push(event);
        }

        assert_eq!(result.output, "hello");
        assert_eq!(observed.len(), 4);
        assert!(observed
            .iter()
            .enumerate()
            .all(|(index, event)| event.sequence == index as u64));
        assert!(matches!(
            observed[0].payload,
            LlmProtocolEventPayload::Started { .. }
        ));
        assert!(matches!(
            observed.last().unwrap().payload,
            LlmProtocolEventPayload::Completed { .. }
        ));
        let deltas = observed
            .iter()
            .filter_map(|event| match &event.payload {
                LlmProtocolEventPayload::Delta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(deltas, "hello");
    }

    #[tokio::test]
    async fn exposes_capabilities_health_and_normalized_model_metadata() {
        let tags = r#"{"models":[{"name":"qwen2.5-coder:14b","model":"qwen2.5-coder:14b","size":9123,"digest":"abc123","details":{"family":"qwen2","parameter_size":"14.8B","quantization_level":"Q4_K_M"}}]}"#;
        let (url, requests) = spawn_sequence_mock(vec![tags, tags]);
        let provider = OllamaProvider::try_new(url).unwrap();

        let capabilities = provider.capabilities();
        assert!(capabilities.local_only);
        assert!(capabilities.streaming);
        assert!(capabilities.cancellation);

        let health = provider
            .health(HealthRequest {
                model: Some("qwen2.5-coder:14b".to_string()),
                ..HealthRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.model_available, Some(true));
        assert_eq!(health.model_count, 1);

        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "qwen2.5-coder:14b");
        assert_eq!(models[0].quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(models[0].parameter_size.as_deref(), Some("14.8B"));
        assert!(requests.recv().unwrap().starts_with("GET /api/tags "));
        assert!(requests.recv().unwrap().starts_with("GET /api/tags "));
    }

    #[tokio::test]
    async fn malformed_ndjson_emits_one_failed_terminal_event() {
        let body = concat!(
            "{\"response\":\"partial\",\"done\":false}\n",
            "this is not json\n"
        );
        let (url, _request) = spawn_stream_mock(body, Duration::ZERO);
        let provider = OllamaProvider::try_new(url).unwrap();
        let (events, mut receiver) = event_channel(8).unwrap();

        let error = provider
            .stream(
                GenerationRequest::new("malformed-1", "qwen", "prompt"),
                events,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        let mut observed = Vec::new();
        while let Some(event) = receiver.recv().await {
            observed.push(event);
        }

        assert_eq!(error.kind, ProviderErrorKind::Protocol);
        assert_eq!(
            observed.iter().filter(|event| event.is_terminal()).count(),
            1
        );
        assert!(matches!(
            observed.last().unwrap().payload,
            LlmProtocolEventPayload::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn in_flight_cancellation_emits_one_cancelled_terminal_event() {
        let url = spawn_cancellable_stream_mock();
        let provider = OllamaProvider::try_new(url).unwrap();
        let (events, mut receiver) = event_channel(8).unwrap();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            provider
                .stream(
                    GenerationRequest::new("cancel-live-1", "qwen", "prompt"),
                    events,
                    task_cancellation,
                )
                .await
        });

        assert!(receiver.recv().await.unwrap().is_started());
        assert_eq!(
            receiver.recv().await.unwrap().output_delta(),
            Some("partial")
        );
        cancellation.cancel();
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        let terminal = receiver.recv().await.unwrap();

        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert_eq!(terminal.sequence, 2);
        assert!(matches!(
            terminal.payload,
            LlmProtocolEventPayload::Cancelled { .. }
        ));
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn cancellation_is_terminal_and_does_not_contact_the_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let provider = OllamaProvider::try_new(format!("http://{address}")).unwrap();
        let (events, mut receiver) = event_channel(8).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = provider
            .stream(
                GenerationRequest::new("cancel-1", "qwen", "prompt"),
                events,
                cancellation,
            )
            .await
            .unwrap_err();
        let mut observed = Vec::new();
        while let Some(event) = receiver.recv().await {
            observed.push(event);
        }

        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert!(observed.last().is_some_and(|event| matches!(
            event.payload,
            LlmProtocolEventPayload::Cancelled { .. }
        )));
    }

    #[tokio::test]
    async fn non_streaming_generation_honors_pre_cancellation() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let provider = OllamaProvider::try_new(format!("http://{address}")).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = provider
            .generate(
                GenerationRequest::new("cancel-generate-1", "qwen", "prompt"),
                cancellation,
            )
            .await
            .unwrap_err();

        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert_eq!(error.stage, "generation_start");
    }

    fn spawn_mock(body: &'static str, delay: Duration) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            sender.send(read_http_request(&mut stream)).unwrap();
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{address}"), receiver)
    }

    fn spawn_status_mock(status: &'static str, body: String) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            sender.send(read_http_request(&mut stream)).unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{address}"), receiver)
    }

    fn spawn_stream_mock(body: &'static str, delay: Duration) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            sender.send(read_http_request(&mut stream)).unwrap();
            let header = "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            stream.write_all(header.as_bytes()).unwrap();
            for piece in body.as_bytes().chunks(17) {
                thread::sleep(delay);
                let chunk_header = format!("{:X}\r\n", piece.len());
                stream.write_all(chunk_header.as_bytes()).unwrap();
                stream.write_all(piece).unwrap();
                stream.write_all(b"\r\n").unwrap();
                stream.flush().unwrap();
            }
            stream.write_all(b"0\r\n\r\n").unwrap();
        });
        (format!("http://{address}"), receiver)
    }

    fn spawn_sequence_mock(bodies: Vec<&'static str>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for body in bodies {
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

    fn spawn_cancellable_stream_mock() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            read_http_request(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let first = b"{\"response\":\"partial\",\"done\":false}\n";
            stream
                .write_all(format!("{:X}\r\n", first.len()).as_bytes())
                .unwrap();
            stream.write_all(first).unwrap();
            stream.write_all(b"\r\n").unwrap();
            stream.flush().unwrap();
            thread::sleep(Duration::from_millis(500));
            let _ = stream.write_all(b"0\r\n\r\n");
        });
        format!("http://{address}")
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
