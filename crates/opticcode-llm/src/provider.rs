use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::{
    GenerationRequest, GenerationResult, HealthReport, HealthRequest, LlmProtocolEvent, ModelInfo,
    ProviderCapabilities, ProviderError, ProviderId,
};

pub use tokio_util::sync::CancellationToken;

pub type EventSink = mpsc::Sender<LlmProtocolEvent>;
pub type EventReceiver = mpsc::Receiver<LlmProtocolEvent>;
pub const MAX_EVENT_CHANNEL_CAPACITY: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventChannelError {
    pub capacity: usize,
}

impl std::fmt::Display for EventChannelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "event channel capacity must be between 1 and {MAX_EVENT_CHANNEL_CAPACITY}, received {}",
            self.capacity
        )
    }
}

impl std::error::Error for EventChannelError {}

pub fn event_channel(capacity: usize) -> Result<(EventSink, EventReceiver), EventChannelError> {
    if capacity == 0 || capacity > MAX_EVENT_CHANNEL_CAPACITY {
        return Err(EventChannelError { capacity });
    }
    Ok(mpsc::channel(capacity))
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn endpoint(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;

    async fn health(&self, request: HealthRequest) -> Result<HealthReport, ProviderError>;

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    async fn generate(
        &self,
        request: GenerationRequest,
        cancellation: CancellationToken,
    ) -> Result<GenerationResult, ProviderError>;

    async fn stream(
        &self,
        request: GenerationRequest,
        events: EventSink,
        cancellation: CancellationToken,
    ) -> Result<GenerationResult, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::{event_channel, MAX_EVENT_CHANNEL_CAPACITY};

    #[test]
    fn event_channels_are_explicitly_bounded() {
        assert!(event_channel(1).is_ok());
        assert!(event_channel(MAX_EVENT_CHANNEL_CAPACITY).is_ok());
        assert!(event_channel(0).is_err());
        assert!(event_channel(MAX_EVENT_CHANNEL_CAPACITY + 1).is_err());
    }
}
