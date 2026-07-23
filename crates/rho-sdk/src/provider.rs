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
    CompactionOutput, ProviderError, ProviderErrorKind, Retryability,
};

/// Future returned by [`ModelProvider`] operations.
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ModelResponse, ProviderError>> + Send + 'a>>;

/// One physical request that failed before native compaction finished or retried.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NativeCompactionFailedAttempt {
    /// Classification of the failed physical request.
    pub kind: ProviderErrorKind,
    /// Usage observed on the failed attempt, when any was reported.
    pub usage: crate::model::ModelUsage,
}

impl NativeCompactionFailedAttempt {
    pub fn new(kind: ProviderErrorKind, usage: crate::model::ModelUsage) -> Self {
        Self { kind, usage }
    }
}

/// Result of one provider-native compaction call, including failed physical attempts.
#[derive(Clone, Debug)]
pub struct NativeCompactionResponse {
    result: Result<CompactionOutput, ProviderError>,
    failed_attempts: Vec<NativeCompactionFailedAttempt>,
}

impl NativeCompactionResponse {
    /// Successful native compaction with no prior failed physical attempts.
    pub fn success(output: CompactionOutput) -> Self {
        Self {
            result: Ok(output),
            failed_attempts: Vec::new(),
        }
    }

    /// Failed native compaction with no prior failed physical attempts.
    pub fn failure(error: ProviderError) -> Self {
        Self {
            result: Err(error),
            failed_attempts: Vec::new(),
        }
    }

    /// Attaches failed physical attempts observed before the final result.
    pub fn with_failed_attempts(
        mut self,
        failed_attempts: impl IntoIterator<Item = NativeCompactionFailedAttempt>,
    ) -> Self {
        self.failed_attempts.extend(failed_attempts);
        self
    }

    pub fn failed_attempts(&self) -> &[NativeCompactionFailedAttempt] {
        &self.failed_attempts
    }

    pub fn into_parts(
        self,
    ) -> (
        Result<CompactionOutput, ProviderError>,
        Vec<NativeCompactionFailedAttempt>,
    ) {
        (self.result, self.failed_attempts)
    }

    pub fn result(&self) -> Result<&CompactionOutput, &ProviderError> {
        self.result.as_ref()
    }
}

impl From<CompactionOutput> for NativeCompactionResponse {
    fn from(output: CompactionOutput) -> Self {
        Self::success(output)
    }
}

impl From<Result<CompactionOutput, ProviderError>> for NativeCompactionResponse {
    fn from(result: Result<CompactionOutput, ProviderError>) -> Self {
        match result {
            Ok(output) => Self::success(output),
            Err(error) => Self::failure(error),
        }
    }
}

/// Future returned by optional provider-native compaction.
pub type NativeCompactionFuture<'a> =
    Pin<Box<dyn Future<Output = NativeCompactionResponse> + Send + 'a>>;

/// Sending side of a bounded provider-event channel.
#[derive(Clone, Debug)]
pub struct ProviderEventSender {
    sender: mpsc::Sender<ProviderStreamEvent>,
}

/// Internal lifecycle event for a physical provider request.
///
/// This type is public only so application provider adapters can forward built-in
/// transport retry boundaries. It is not part of the semantic model event stream.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderRequestEvent {
    /// A physical request failed before the provider retried internally.
    RequestAttemptFailed {
        kind: ProviderErrorKind,
        usage: crate::model::ModelUsage,
    },
}

/// An item from either provider event path.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderStreamEvent {
    Model(ModelEvent),
    Request(ProviderRequestEvent),
}

impl ProviderEventSender {
    /// Returns the fixed capacity configured for this event stream.
    pub fn capacity(&self) -> usize {
        self.sender.max_capacity()
    }

    /// Sends an event, waiting for bounded channel capacity when necessary.
    pub async fn send(&self, event: ModelEvent) -> Result<(), ProviderError> {
        self.sender
            .send(ProviderStreamEvent::Model(event))
            .await
            .map_err(|_| ProviderError::interrupted("provider event consumer was dropped"))
    }

    /// Reports a failed physical request that the provider will retry internally.
    #[doc(hidden)]
    pub async fn send_request_attempt_failed(
        &self,
        kind: ProviderErrorKind,
        usage: crate::model::ModelUsage,
    ) -> Result<(), ProviderError> {
        self.sender
            .send(ProviderStreamEvent::Request(
                ProviderRequestEvent::RequestAttemptFailed { kind, usage },
            ))
            .await
            .map_err(|_| ProviderError::interrupted("provider request event consumer was dropped"))
    }
}

/// Receiving side of a bounded provider-event channel.
#[derive(Debug)]
pub struct ProviderEventReceiver {
    receiver: mpsc::Receiver<ProviderStreamEvent>,
    pending_model_events: VecDeque<ModelEvent>,
    pending_request_events: VecDeque<ProviderRequestEvent>,
}

impl ProviderEventReceiver {
    /// Receives the next event, or `None` after every sender is dropped.
    pub async fn recv(&mut self) -> Option<ModelEvent> {
        if let Some(event) = self.pending_model_events.pop_front() {
            return Some(event);
        }
        while let Some(event) = self.receiver.recv().await {
            match event {
                ProviderStreamEvent::Model(event) => return Some(event),
                ProviderStreamEvent::Request(event) => self.pending_request_events.push_back(event),
            }
        }
        None
    }

    /// Receives the next physical request lifecycle event.
    #[doc(hidden)]
    pub async fn recv_request_event(&mut self) -> Option<ProviderRequestEvent> {
        if let Some(event) = self.pending_request_events.pop_front() {
            return Some(event);
        }
        while let Some(event) = self.receiver.recv().await {
            match event {
                ProviderStreamEvent::Request(event) => return Some(event),
                ProviderStreamEvent::Model(event) => self.pending_model_events.push_back(event),
            }
        }
        None
    }

    /// Receives the next semantic or physical request event.
    #[doc(hidden)]
    pub async fn recv_stream_event(&mut self) -> Option<ProviderStreamEvent> {
        self.receiver.recv().await
    }

    pub(crate) fn try_recv_stream_event(&mut self) -> Option<ProviderStreamEvent> {
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
        ProviderEventReceiver {
            receiver,
            pending_model_events: VecDeque::new(),
            pending_request_events: VecDeque::new(),
        },
    )
}

/// How provider cancellation is finalized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderCancellationMode {
    /// The SDK must drop the provider future to guarantee cancellation.
    External,
    /// The provider cooperatively stops after forwarding already accepted events.
    Cooperative,
}

/// Extension point for provider-neutral model backends.
///
/// Implementors must not mutate session history. They receive an immutable
/// request snapshot, must cooperate with its cancellation token, and must keep
/// provider-native replay data scoped to [`ModelIdentity`]. Returned futures
/// must be `Send` so hosts may drive them on multithreaded executors.
pub trait ModelProvider: Send + Sync {
    /// Declares whether cancellation must drop the future or await cooperative cleanup.
    fn cancellation_mode(&self) -> ProviderCancellationMode {
        ProviderCancellationMode::External
    }

    /// Exact identity used to scope provider-native replay data.
    fn identity(&self) -> ModelIdentity;

    /// Completes one model turn without streaming intermediate events.
    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a>;

    /// Optional provider-native history compaction.
    ///
    /// Returns `None` when the provider has no native compaction path. When
    /// `Some`, the future must return complete replacement history suitable for
    /// session commit, and must cooperate with the request cancellation token.
    fn native_compact<'a>(
        &'a self,
        _request: ModelRequest<'a>,
    ) -> Option<NativeCompactionFuture<'a>> {
        None
    }

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
    events: Vec<ProviderStreamEvent>,
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
            events: events.into_iter().map(ProviderStreamEvent::Model).collect(),
            result: Ok(response),
        }
    }

    /// Creates a failed turn that emitted semantic events before the failure.
    #[doc(hidden)]
    pub fn streaming_failed(events: Vec<ModelEvent>, error: ProviderError) -> Self {
        Self {
            events: events.into_iter().map(ProviderStreamEvent::Model).collect(),
            result: Err(error),
        }
    }

    /// Creates a turn with semantic and physical request events.
    #[doc(hidden)]
    pub fn streaming_with_request_events(
        events: Vec<ProviderStreamEvent>,
        response: ModelResponse,
    ) -> Self {
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
    native_compactions: Arc<Mutex<VecDeque<NativeCompactionResponse>>>,
}

impl ScriptedProvider {
    pub fn new(identity: ModelIdentity, turns: impl IntoIterator<Item = ScriptedTurn>) -> Self {
        Self {
            identity,
            turns: Arc::new(Mutex::new(turns.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
            native_compactions: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Queues provider-native compaction results for later [`ModelProvider::native_compact`] calls.
    pub fn with_native_compactions(
        mut self,
        outputs: impl IntoIterator<Item = impl Into<NativeCompactionResponse>>,
    ) -> Self {
        self.native_compactions =
            Arc::new(Mutex::new(outputs.into_iter().map(Into::into).collect()));
        self
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

    fn take_native_compaction(
        &self,
        request: &ModelRequest<'_>,
    ) -> Option<NativeCompactionResponse> {
        let mut queue = self
            .native_compactions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if queue.is_empty() {
            return None;
        }
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(RecordedModelRequest {
                messages: request.messages.to_vec(),
                tools: request.tools.to_vec(),
                reasoning_level: request.reasoning_level,
                prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
            });
        queue.pop_front()
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

    fn native_compact<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> Option<NativeCompactionFuture<'a>> {
        let response = self.take_native_compaction(&request)?;
        Some(Box::pin(async move {
            if request.cancellation.is_cancelled() {
                return NativeCompactionResponse::failure(ProviderError::interrupted(
                    "provider request cancelled",
                ));
            }
            // Cooperative cancellation for tests that cancel after the future starts.
            if !request.cancellation.is_cancelled() {
                tokio::task::yield_now().await;
            }
            if request.cancellation.is_cancelled() {
                return NativeCompactionResponse::failure(ProviderError::interrupted(
                    "provider request cancelled",
                ));
            }
            response
        }))
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
                    result = async {
                        match event {
                            ProviderStreamEvent::Model(event) => events.send(event).await,
                            ProviderStreamEvent::Request(
                                ProviderRequestEvent::RequestAttemptFailed { kind, usage },
                            ) => events.send_request_attempt_failed(kind, usage).await,
                        }
                    } => result?,
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
