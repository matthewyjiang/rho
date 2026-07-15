use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
};

use tokio::sync::mpsc;

use crate::{
    model::{ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
    ProviderError, ProviderErrorKind, Retryability,
};

/// Future returned by [`ModelProvider`] operations.
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ModelResponse, ProviderError>> + Send + 'a>>;

/// Sending side of a bounded provider-event channel.
#[derive(Clone, Debug)]
pub struct ProviderEventSender {
    sender: mpsc::Sender<ModelEvent>,
}

impl ProviderEventSender {
    /// Sends an event, waiting for bounded channel capacity when necessary.
    pub async fn send(&self, event: ModelEvent) -> Result<(), ProviderError> {
        self.sender
            .send(event)
            .await
            .map_err(|_| ProviderError::interrupted("provider event consumer was dropped"))
    }
}

/// Receiving side of a bounded provider-event channel.
#[derive(Debug)]
pub struct ProviderEventReceiver {
    receiver: mpsc::Receiver<ModelEvent>,
}

impl ProviderEventReceiver {
    /// Receives the next event, or `None` after every sender is dropped.
    pub async fn recv(&mut self) -> Option<ModelEvent> {
        self.receiver.recv().await
    }

    pub(crate) fn try_recv(&mut self) -> Option<ModelEvent> {
        self.receiver.try_recv().ok()
    }
}

/// Creates a bounded provider-event channel with explicit backpressure.
pub fn provider_event_channel(
    capacity: NonZeroUsize,
) -> (ProviderEventSender, ProviderEventReceiver) {
    let (sender, receiver) = mpsc::channel(capacity.get());
    (
        ProviderEventSender { sender },
        ProviderEventReceiver { receiver },
    )
}

/// Extension point for provider-neutral model backends.
///
/// Implementors must not mutate session history. They receive an immutable
/// request snapshot, must cooperate with its cancellation token, and must keep
/// provider-native replay data scoped to [`ModelIdentity`]. Returned futures
/// must be `Send` so hosts may drive them on multithreaded executors.
pub trait ModelProvider: Send + Sync {
    /// Exact identity used to scope provider-native replay data.
    fn identity(&self) -> ModelIdentity;

    /// Completes one model turn without streaming intermediate events.
    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a>;

    /// Completes one model turn while sending semantic events in order.
    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            let cancellation = request.cancellation.clone();
            let response = tokio::select! {
                response = self.send_turn(request) => response?,
                () = cancellation.cancelled() => {
                    return Err(ProviderError::interrupted("provider request cancelled"));
                }
            };
            let ModelResponse::Assistant(blocks) = &response;
            for block in blocks {
                if let crate::model::ContentBlock::Text(text) = block {
                    events.send(ModelEvent::OutputDelta(text.clone())).await?;
                }
            }
            Ok(response)
        })
    }
}

/// Owned request snapshot captured by [`ScriptedProvider`].
///
/// Fields are readable for assertions, while the non-exhaustive marker reserves
/// space for future request metadata. Downstream code receives this value from
/// [`ScriptedProvider::recorded_requests`] rather than constructing it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RecordedModelRequest {
    pub messages: Vec<crate::model::Message>,
    pub tools: Vec<crate::model::ToolSpec>,
    pub reasoning_level: crate::ReasoningLevel,
    pub prompt_cache_key: Option<String>,
}

/// One deterministic turn returned by [`ScriptedProvider`].
#[derive(Clone, Debug)]
pub struct ScriptedTurn {
    events: Vec<ModelEvent>,
    result: Result<ModelResponse, ProviderError>,
}

impl ScriptedTurn {
    pub fn completed(response: ModelResponse) -> Self {
        Self {
            events: Vec::new(),
            result: Ok(response),
        }
    }

    pub fn streaming(events: Vec<ModelEvent>, response: ModelResponse) -> Self {
        Self {
            events,
            result: Ok(response),
        }
    }

    pub fn failed(error: ProviderError) -> Self {
        Self {
            events: Vec::new(),
            result: Err(error),
        }
    }
}

/// Deterministic provider for downstream tests and examples.
#[derive(Clone)]
pub struct ScriptedProvider {
    identity: ModelIdentity,
    turns: Arc<Mutex<VecDeque<ScriptedTurn>>>,
    requests: Arc<Mutex<Vec<RecordedModelRequest>>>,
}

impl ScriptedProvider {
    pub fn new(identity: ModelIdentity, turns: impl IntoIterator<Item = ScriptedTurn>) -> Self {
        Self {
            identity,
            turns: Arc::new(Mutex::new(turns.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn recorded_requests(&self) -> Vec<RecordedModelRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn take_turn(&self, request: &ModelRequest<'_>) -> Result<ScriptedTurn, ProviderError> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(RecordedModelRequest {
                messages: request.messages.to_vec(),
                tools: request.tools.to_vec(),
                reasoning_level: request.reasoning_level,
                prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
            });
        self.turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::InvalidResponse,
                    "scripted provider has no remaining turn",
                    Retryability::Permanent,
                )
            })
    }
}

impl fmt::Debug for ScriptedProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptedProvider")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl ModelProvider for ScriptedProvider {
    fn identity(&self) -> ModelIdentity {
        self.identity.clone()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Err(ProviderError::interrupted("provider request cancelled"));
            }
            self.take_turn(&request)?.result
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return Err(ProviderError::interrupted("provider request cancelled"));
            }
            let cancellation = request.cancellation.clone();
            let turn = self.take_turn(&request)?;
            for event in turn.events {
                tokio::select! {
                    result = events.send(event) => result?,
                    () = cancellation.cancelled() => {
                        return Err(ProviderError::interrupted("provider request cancelled"));
                    }
                }
            }
            turn.result
        })
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
