use std::fmt;

use serde::{Deserialize, Serialize};

pub const LLM_PROTOCOL_ID: &str = "opticcode.llm";
pub const LLM_PROTOCOL_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_PROVIDER_TIMEOUT_MS: u64 = 120_000;
pub const MAX_PROVIDER_TIMEOUT_MS: u64 = 3_600_000;
pub const MAX_REQUEST_ID_BYTES: usize = 128;
pub const MAX_MODEL_NAME_BYTES: usize = 256;
pub const MAX_PROMPT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_GENERATED_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Ollama,
}

impl ProviderId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub local_only: bool,
    pub health: bool,
    pub model_listing: bool,
    pub generation: bool,
    pub streaming: bool,
    pub cancellation: bool,
    pub token_usage: bool,
    pub provider_timings: bool,
    pub deterministic_seed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthRequest {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub timeout_ms: u64,
}

impl Default for HealthRequest {
    fn default() -> Self {
        Self {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            model: None,
            timeout_ms: DEFAULT_PROVIDER_TIMEOUT_MS,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    ModelUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthReport {
    pub schema_version: u32,
    pub provider: ProviderId,
    pub endpoint: String,
    pub status: HealthStatus,
    pub reachable: bool,
    pub latency_ms: u64,
    pub model_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_available: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub schema_version: u32,
    pub provider: ProviderId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenerationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<String>,
    pub timeout_ms: u64,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            max_output_tokens: None,
            temperature: None,
            seed: None,
            keep_alive: None,
            timeout_ms: DEFAULT_PROVIDER_TIMEOUT_MS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenerationRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub options: GenerationOptions,
}

impl GenerationRequest {
    pub fn new(
        request_id: impl Into<String>,
        model: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.into(),
            model: model.into(),
            prompt: prompt.into(),
            options: GenerationOptions::default(),
        }
    }

    pub fn validate(&self, provider: ProviderId) -> Result<(), ProviderError> {
        validate_schema(self.schema_version, provider, "generation_request")?;
        validate_request_id(&self.request_id, provider)?;
        validate_model_name(&self.model, provider)?;
        if self.prompt.is_empty() {
            return Err(ProviderError::invalid_request(
                provider,
                "request_validation",
                "generation prompt must not be empty",
            ));
        }
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(ProviderError::invalid_request(
                provider,
                "request_validation",
                format!(
                    "generation prompt exceeds the {} byte protocol limit",
                    MAX_PROMPT_BYTES
                ),
            ));
        }
        if self.options.max_output_tokens == Some(0) {
            return Err(ProviderError::invalid_request(
                provider,
                "request_validation",
                "maximum output tokens must be greater than zero",
            ));
        }
        if self
            .options
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        {
            return Err(ProviderError::invalid_request(
                provider,
                "request_validation",
                "temperature must be a finite value between 0 and 2",
            ));
        }
        validate_timeout(self.options.timeout_ms, provider, "request_validation")?;
        if self.options.keep_alive.as_ref().is_some_and(|value| {
            value.len() > 64 || value.chars().any(|character| character.is_control())
        }) {
            return Err(ProviderError::invalid_request(
                provider,
                "request_validation",
                "keep_alive must be at most 64 bytes and contain no control characters",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    Unloaded,
    Unknown,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationTimings {
    pub client_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_total_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_eval_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerationResult {
    pub schema_version: u32,
    pub request_id: String,
    pub provider: ProviderId,
    pub model: String,
    pub output: String,
    pub finish_reason: FinishReason,
    pub prompt_chars: usize,
    pub usage: GenerationUsage,
    pub timings: GenerationTimings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    InvalidConfiguration,
    InvalidRequest,
    Unavailable,
    ModelUnavailable,
    Timeout,
    HttpStatus,
    Protocol,
    Cancelled,
    EventSinkClosed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderError {
    pub schema_version: u32,
    pub provider: ProviderId,
    pub kind: ProviderErrorKind,
    pub stage: String,
    pub retryable: bool,
    pub message: String,
}

impl ProviderError {
    pub fn new(
        provider: ProviderId,
        kind: ProviderErrorKind,
        stage: impl Into<String>,
        retryable: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            provider,
            kind,
            stage: stage.into(),
            retryable,
            message: message.into(),
        }
    }

    pub fn invalid_configuration(
        provider: ProviderId,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            provider,
            ProviderErrorKind::InvalidConfiguration,
            stage,
            false,
            message,
        )
    }

    pub fn invalid_request(
        provider: ProviderId,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            provider,
            ProviderErrorKind::InvalidRequest,
            stage,
            false,
            message,
        )
    }

    pub fn cancelled(provider: ProviderId, stage: impl Into<String>) -> Self {
        Self::new(
            provider,
            ProviderErrorKind::Cancelled,
            stage,
            false,
            "generation was cancelled",
        )
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} provider error at {} ({:?}): {}",
            self.provider, self.stage, self.kind, self.message
        )
    }
}

impl std::error::Error for ProviderError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmProtocolEvent {
    pub schema_version: u32,
    pub protocol: String,
    pub request_id: String,
    pub sequence: u64,
    #[serde(flatten)]
    pub payload: LlmProtocolEventPayload,
}

impl LlmProtocolEvent {
    pub fn new(
        request_id: impl Into<String>,
        sequence: u64,
        payload: LlmProtocolEventPayload,
    ) -> Self {
        Self {
            schema_version: LLM_PROTOCOL_SCHEMA_VERSION,
            protocol: LLM_PROTOCOL_ID.to_string(),
            request_id: request_id.into(),
            sequence,
            payload,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.payload,
            LlmProtocolEventPayload::Completed { .. }
                | LlmProtocolEventPayload::Failed { .. }
                | LlmProtocolEventPayload::Cancelled { .. }
        )
    }

    pub fn is_started(&self) -> bool {
        matches!(self.payload, LlmProtocolEventPayload::Started { .. })
    }

    pub fn output_delta(&self) -> Option<&str> {
        match &self.payload {
            LlmProtocolEventPayload::Delta { text } => Some(text),
            _ => None,
        }
    }

    pub fn validate(
        &self,
        expected_provider: ProviderId,
        expected_request_id: &str,
        expected_model: &str,
        expected_sequence: u64,
    ) -> Result<(), ProviderError> {
        validate_schema(self.schema_version, expected_provider, "event_validation")?;
        if self.protocol != LLM_PROTOCOL_ID {
            return Err(ProviderError::new(
                expected_provider,
                ProviderErrorKind::Protocol,
                "event_validation",
                false,
                format!("unexpected LLM protocol identifier `{}`", self.protocol),
            ));
        }
        validate_request_id(&self.request_id, expected_provider)?;
        if self.request_id != expected_request_id {
            return Err(ProviderError::new(
                expected_provider,
                ProviderErrorKind::Protocol,
                "event_validation",
                false,
                "LLM event request_id does not match the active request",
            ));
        }
        if self.sequence != expected_sequence {
            return Err(ProviderError::new(
                expected_provider,
                ProviderErrorKind::Protocol,
                "event_validation",
                false,
                format!(
                    "LLM event sequence mismatch: expected {expected_sequence}, received {}",
                    self.sequence
                ),
            ));
        }
        match &self.payload {
            LlmProtocolEventPayload::Started { provider, model }
                if *provider != expected_provider || model != expected_model =>
            {
                Err(ProviderError::new(
                    expected_provider,
                    ProviderErrorKind::Protocol,
                    "event_validation",
                    false,
                    "LLM started event identifies a different provider or model",
                ))
            }
            LlmProtocolEventPayload::Completed { result }
                if result.schema_version != LLM_PROTOCOL_SCHEMA_VERSION
                    || result.request_id != expected_request_id
                    || result.provider != expected_provider
                    || result.model != expected_model
                    || result.output.len() > MAX_GENERATED_OUTPUT_BYTES =>
            {
                Err(ProviderError::new(
                    expected_provider,
                    ProviderErrorKind::Protocol,
                    "event_validation",
                    false,
                    "LLM completed event contains an inconsistent generation result",
                ))
            }
            LlmProtocolEventPayload::Failed { error }
                if error.schema_version != LLM_PROTOCOL_SCHEMA_VERSION
                    || error.provider != expected_provider =>
            {
                Err(ProviderError::new(
                    expected_provider,
                    ProviderErrorKind::Protocol,
                    "event_validation",
                    false,
                    "LLM failed event contains an inconsistent provider error",
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmProtocolEventPayload {
    Started { provider: ProviderId, model: String },
    Delta { text: String },
    Completed { result: GenerationResult },
    Failed { error: ProviderError },
    Cancelled { reason: String },
}

pub fn validate_schema(
    schema_version: u32,
    provider: ProviderId,
    stage: &str,
) -> Result<(), ProviderError> {
    if schema_version != LLM_PROTOCOL_SCHEMA_VERSION {
        return Err(ProviderError::invalid_request(
            provider,
            stage,
            format!(
                "unsupported LLM protocol schema {schema_version}; expected {}",
                LLM_PROTOCOL_SCHEMA_VERSION
            ),
        ));
    }
    Ok(())
}

pub fn validate_timeout(
    timeout_ms: u64,
    provider: ProviderId,
    stage: &str,
) -> Result<(), ProviderError> {
    if timeout_ms == 0 || timeout_ms > MAX_PROVIDER_TIMEOUT_MS {
        return Err(ProviderError::invalid_request(
            provider,
            stage,
            format!(
                "provider timeout must be between 1 and {} milliseconds",
                MAX_PROVIDER_TIMEOUT_MS
            ),
        ));
    }
    Ok(())
}

pub fn validate_request_id(value: &str, provider: ProviderId) -> Result<(), ProviderError> {
    if value.is_empty()
        || value.len() > MAX_REQUEST_ID_BYTES
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)))
    {
        return Err(ProviderError::invalid_request(
            provider,
            "request_validation",
            "request_id must contain 1-128 ASCII letters, digits, '-', '_', '.' or ':'",
        ));
    }
    Ok(())
}

pub fn validate_model_name(value: &str, provider: ProviderId) -> Result<(), ProviderError> {
    if value.trim().is_empty()
        || value.len() > MAX_MODEL_NAME_BYTES
        || value.chars().any(|character| character.is_control())
    {
        return Err(ProviderError::invalid_request(
            provider,
            "request_validation",
            "model name must contain 1-256 bytes and no control characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        GenerationRequest, LlmProtocolEvent, LlmProtocolEventPayload, ProviderError, ProviderId,
        LLM_PROTOCOL_SCHEMA_VERSION,
    };

    #[test]
    fn generation_request_validation_is_bounded_and_versioned() {
        let request = GenerationRequest::new("request-1", "qwen:14b", "hello");
        assert!(request.validate(ProviderId::Ollama).is_ok());

        let mut invalid = request.clone();
        invalid.schema_version = LLM_PROTOCOL_SCHEMA_VERSION + 1;
        assert!(invalid.validate(ProviderId::Ollama).is_err());
        invalid = request.clone();
        invalid.request_id = "not allowed / path".to_string();
        assert!(invalid.validate(ProviderId::Ollama).is_err());
        invalid = request;
        invalid.options.temperature = Some(f32::NAN);
        assert!(invalid.validate(ProviderId::Ollama).is_err());
    }

    #[test]
    fn protocol_events_round_trip_with_a_stable_tag() {
        let event = LlmProtocolEvent::new(
            "request-1",
            2,
            LlmProtocolEventPayload::Failed {
                error: ProviderError::cancelled(ProviderId::Ollama, "stream"),
            },
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""schema_version":1"#));
        assert!(json.contains(r#""type":"failed""#));
        assert_eq!(
            serde_json::from_str::<LlmProtocolEvent>(&json).unwrap(),
            event
        );
        assert!(event.is_terminal());
    }

    #[test]
    fn event_validation_rejects_cross_request_and_sequence_confusion() {
        let event = LlmProtocolEvent::new(
            "request-1",
            0,
            LlmProtocolEventPayload::Started {
                provider: ProviderId::Ollama,
                model: "qwen".to_string(),
            },
        );
        assert!(event
            .validate(ProviderId::Ollama, "request-1", "qwen", 0)
            .is_ok());
        assert!(event
            .validate(ProviderId::Ollama, "request-2", "qwen", 0)
            .is_err());
        assert!(event
            .validate(ProviderId::Ollama, "request-1", "other", 0)
            .is_err());
        assert!(event
            .validate(ProviderId::Ollama, "request-1", "qwen", 1)
            .is_err());
    }
}
