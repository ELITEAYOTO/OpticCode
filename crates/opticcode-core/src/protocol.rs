use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use opticcode_llm::{CancellationToken, LlmProtocolEvent, ProviderId, MAX_REQUEST_ID_BYTES};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{AssistantCommandKind, AssistantCommandReport, AssistantStructuredError, ContextMode};

pub const ASSISTANT_PROTOCOL_ID: &str = "opticcode.assistant";
pub const ASSISTANT_PROTOCOL_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_ASSISTANT_EVENT_CAPACITY: usize = 64;
pub const MAX_ASSISTANT_REQUEST_ID_BYTES: usize = MAX_REQUEST_ID_BYTES;
const MAX_ASSISTANT_EVENT_CAPACITY: usize = 4_096;
const EVENT_DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

pub type AssistantEventSink = mpsc::Sender<AssistantProtocolEvent>;
pub type AssistantEventReceiver = mpsc::Receiver<AssistantProtocolEvent>;

#[derive(Debug, Clone)]
pub struct AssistantProtocolSession {
    pub request_id: String,
    pub events: AssistantEventSink,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantProtocolEvent {
    pub schema_version: u32,
    pub protocol: String,
    pub request_id: String,
    pub sequence: u64,
    #[serde(flatten)]
    pub payload: AssistantProtocolEventPayload,
}

impl AssistantProtocolEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.payload,
            AssistantProtocolEventPayload::Completed { .. }
                | AssistantProtocolEventPayload::Failed { .. }
                | AssistantProtocolEventPayload::Cancelled { .. }
        )
    }

    pub fn output_delta(&self) -> Option<&str> {
        match &self.payload {
            AssistantProtocolEventPayload::ProviderEvent { event, .. } => event.output_delta(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantProtocolEventPayload {
    Started {
        command: AssistantCommandKind,
        provider: ProviderId,
        model: String,
        requested_context_mode: ContextMode,
    },
    ContextPrepared {
        requested_context_mode: ContextMode,
        used_context_mode: Option<ContextMode>,
        analysis_complete: bool,
        fallback_applied: bool,
        variant_count: usize,
    },
    ProviderEvent {
        context_mode: ContextMode,
        event: Box<LlmProtocolEvent>,
    },
    Completed {
        report_schema_version: u32,
        generated_runs: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<Box<AssistantCompletionSummary>>,
    },
    Failed {
        errors: Vec<AssistantStructuredError>,
    },
    Cancelled {
        errors: Vec<AssistantStructuredError>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantCompletionSummary {
    pub command: AssistantCommandKind,
    pub success: bool,
    pub model: String,
    pub requested_context_mode: ContextMode,
    pub used_context_mode: Option<ContextMode>,
    pub preparation_duration_us: u64,
    pub warnings: Vec<String>,
    pub context_files: Vec<AssistantCompletionContextFile>,
    pub runs: Vec<AssistantCompletionRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistantCompletionContextFile {
    pub context_mode: ContextMode,
    pub path: String,
    pub snippets: usize,
    pub max_score: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantCompletionRun {
    pub context_mode: ContextMode,
    pub generated: bool,
    pub estimated_prompt_tokens: usize,
    pub client_ms: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub generated_tokens_per_second: Option<f64>,
}

impl From<&AssistantCommandReport> for AssistantCompletionSummary {
    fn from(report: &AssistantCommandReport) -> Self {
        Self {
            command: report.command,
            success: report.success,
            model: report.model.clone(),
            requested_context_mode: report.requested_context_mode,
            used_context_mode: report.used_context_mode,
            preparation_duration_us: report.preparation_duration_us,
            warnings: report.warnings.clone(),
            context_files: report
                .context
                .variants
                .iter()
                .flat_map(|variant| {
                    variant
                        .report
                        .files
                        .iter()
                        .map(move |file| AssistantCompletionContextFile {
                            context_mode: variant.report.mode,
                            path: file.path.clone(),
                            snippets: file.snippets,
                            max_score: file.max_score,
                        })
                })
                .collect(),
            runs: report
                .runs
                .iter()
                .map(|run| AssistantCompletionRun {
                    context_mode: run.context_mode,
                    generated: run.generated,
                    estimated_prompt_tokens: run.prompt.estimated_tokens,
                    client_ms: run.metrics.as_ref().map(|metrics| metrics.client_ms),
                    prompt_tokens: run
                        .metrics
                        .as_ref()
                        .and_then(|metrics| metrics.prompt_eval_count),
                    generated_tokens: run
                        .metrics
                        .as_ref()
                        .and_then(|metrics| metrics.generated_tokens),
                    generated_tokens_per_second: run
                        .metrics
                        .as_ref()
                        .and_then(|metrics| metrics.generated_tokens_per_second),
                })
                .collect(),
        }
    }
}

pub fn assistant_event_channel(
    capacity: usize,
) -> Result<(AssistantEventSink, AssistantEventReceiver)> {
    if capacity == 0 || capacity > MAX_ASSISTANT_EVENT_CAPACITY {
        bail!(
            "assistant event channel capacity must be between 1 and {}",
            MAX_ASSISTANT_EVENT_CAPACITY
        );
    }
    Ok(mpsc::channel(capacity))
}

pub fn validate_assistant_request_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_ASSISTANT_REQUEST_ID_BYTES
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)))
    {
        bail!(
            "assistant request id must contain 1-{} ASCII letters, digits, '-', '_', '.' or ':'",
            MAX_ASSISTANT_REQUEST_ID_BYTES
        );
    }
    Ok(())
}

pub fn generated_request_id() -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("assistant-{}-{millis}-{sequence}", std::process::id())
}

#[derive(Clone)]
pub(crate) struct AssistantEventEmitter {
    request_id: Arc<str>,
    events: AssistantEventSink,
    sequence: Arc<AtomicU64>,
}

impl AssistantEventEmitter {
    pub(crate) fn new(session: &AssistantProtocolSession) -> Result<Self> {
        validate_assistant_request_id(&session.request_id)?;
        Ok(Self {
            request_id: Arc::from(session.request_id.as_str()),
            events: session.events.clone(),
            sequence: Arc::new(AtomicU64::new(0)),
        })
    }

    pub(crate) async fn send(&self, payload: AssistantProtocolEventPayload) -> Result<()> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let event = AssistantProtocolEvent {
            schema_version: ASSISTANT_PROTOCOL_SCHEMA_VERSION,
            protocol: ASSISTANT_PROTOCOL_ID.to_string(),
            request_id: self.request_id.to_string(),
            sequence,
            payload,
        };
        tokio::time::timeout(EVENT_DELIVERY_TIMEOUT, self.events.send(event))
            .await
            .context("assistant protocol event delivery timed out")?
            .context("assistant protocol event receiver was closed")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assistant_event_channel, validate_assistant_request_id, AssistantEventEmitter,
        AssistantProtocolEventPayload, AssistantProtocolSession,
    };
    use crate::{AssistantCommandKind, ContextMode};
    use opticcode_llm::{CancellationToken, ProviderId};

    #[tokio::test]
    async fn assistant_events_are_ordered_versioned_and_machine_readable() {
        let (events, mut receiver) = assistant_event_channel(4).unwrap();
        let session = AssistantProtocolSession {
            request_id: "ask-1".to_string(),
            events,
            cancellation: CancellationToken::new(),
        };
        let emitter = AssistantEventEmitter::new(&session).unwrap();
        emitter
            .send(AssistantProtocolEventPayload::Started {
                command: AssistantCommandKind::Ask,
                provider: ProviderId::Ollama,
                model: "qwen".to_string(),
                requested_context_mode: ContextMode::Legacy,
            })
            .await
            .unwrap();
        emitter
            .send(AssistantProtocolEventPayload::Completed {
                report_schema_version: 1,
                generated_runs: 1,
                summary: None,
            })
            .await
            .unwrap();
        drop(emitter);
        drop(session);

        let first = receiver.recv().await.unwrap();
        let second = receiver.recv().await.unwrap();
        assert_eq!(first.sequence, 0);
        assert_eq!(second.sequence, 1);
        assert!(second.is_terminal());
        let encoded = serde_json::to_string(&second).unwrap();
        let decoded = serde_json::from_str::<super::AssistantProtocolEvent>(&encoded).unwrap();
        assert_eq!(decoded.sequence, second.sequence);
        assert!(decoded.is_terminal());
        let json = serde_json::to_value(second).unwrap();
        assert_eq!(json["protocol"], "opticcode.assistant");
        assert_eq!(json["type"], "completed");
    }

    #[test]
    fn request_ids_reject_paths_controls_and_unbounded_values() {
        assert!(validate_assistant_request_id("request:ask-1").is_ok());
        assert!(validate_assistant_request_id("../escape").is_err());
        assert!(validate_assistant_request_id(&"x".repeat(129)).is_err());
    }
}
