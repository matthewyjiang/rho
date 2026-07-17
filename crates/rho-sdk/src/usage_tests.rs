use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

use super::*;
use crate::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
    provider::{
        ModelProvider, ProviderEventSender, ProviderFuture, ProviderRequestEvent,
        ProviderStreamEvent, ScriptedProvider, ScriptedTurn,
    },
    Rho, RunEvent, SessionId, SessionOptions, UserInput, Workspace,
};

#[derive(Clone, Default)]
struct CapturingRecorder {
    events: Arc<Mutex<Vec<ProviderRequestUsageEvent>>>,
    failure: Option<ProviderRequestUsageRecorderError>,
}

impl CapturingRecorder {
    fn events(&self) -> Vec<ProviderRequestUsageEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ProviderRequestUsageRecorder for CapturingRecorder {
    fn record(&self, event: ProviderRequestUsageEvent) -> ProviderRequestUsageRecorderFuture<'_> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
        let failure = self.failure.clone();
        Box::pin(async move { failure.map_or(Ok(()), Err) })
    }
}

fn identity() -> ModelIdentity {
    ModelIdentity::new("provider-exact", "api-exact", "model-exact")
}

struct QueuedUsageBeforeCancellationProvider {
    usage: ModelUsage,
    queue_observed: Mutex<Option<oneshot::Sender<()>>>,
}

impl ModelProvider for QueuedUsageBeforeCancellationProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(crate::ProviderError::interrupted("cancelled"))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        let usage = self.usage.clone();
        let queue_observed = self
            .queue_observed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap();
        tokio::spawn(async move {
            events
                .send(ModelEvent::OutputDelta("partial".into()))
                .await
                .unwrap();
            events.send(ModelEvent::Usage(usage)).await.unwrap();
            for index in 0..14 {
                events
                    .send(ModelEvent::WebSearch(format!("queued {index}")))
                    .await
                    .unwrap();
            }
            // The channel is full. This send can finish only after orchestration
            // has received OutputDelta and blocked publishing it to the host.
            events
                .send(ModelEvent::WebSearch("capacity probe".into()))
                .await
                .unwrap();
            let _ = queue_observed.send(());
        });
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(crate::ProviderError::interrupted("cancelled"))
        })
    }
}

#[tokio::test]
async fn cancellation_records_queued_usage_before_usage_updated_is_emitted() {
    let usage = ModelUsage {
        output_tokens: Some(5),
        cost_usd_micros: Some(17),
        ..ModelUsage::default()
    };
    let recorder = CapturingRecorder::default();
    let (queue_observed, queued) = oneshot::channel();
    let rho = Rho::builder()
        .provider(QueuedUsageBeforeCancellationProvider {
            usage: usage.clone(),
            queue_observed: Mutex::new(Some(queue_observed)),
        })
        .usage_recorder(recorder.clone())
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = rho.session(SessionOptions::new()).await.unwrap();
    let mut run = session.start(UserInput::text("go")).await.unwrap();

    assert!(matches!(
        run.next_event().await,
        Some(RunEvent::Started { .. })
    ));
    queued.await.unwrap();
    run.cancel();

    let mut usage_updated = false;
    while let Some(event) = run.next_event().await {
        usage_updated |= matches!(event, RunEvent::UsageUpdated { .. });
        if matches!(event, RunEvent::Cancelled { .. }) {
            break;
        }
    }
    assert!(!usage_updated);
    assert!(matches!(run.outcome().await, Err(crate::Error::Cancelled)));

    let events = recorder.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome(), ProviderRequestOutcome::Cancelled);
    assert_eq!(events[0].usage(), &usage);
}

struct UsageThenWaitProvider {
    usage: ModelUsage,
}

impl ModelProvider for UsageThenWaitProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            request.cancellation.cancelled().await;
            Err(crate::ProviderError::interrupted("cancelled"))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            events.send(ModelEvent::Usage(self.usage.clone())).await?;
            request.cancellation.cancelled().await;
            Err(crate::ProviderError::interrupted("cancelled"))
        })
    }
}

#[tokio::test]
async fn cancellation_records_usage_observed_before_interruption() {
    let usage = ModelUsage {
        output_tokens: Some(3),
        cost_usd_micros: Some(9),
        ..ModelUsage::default()
    };
    let recorder = CapturingRecorder::default();
    let rho = Rho::builder()
        .provider(UsageThenWaitProvider {
            usage: usage.clone(),
        })
        .usage_recorder(recorder.clone())
        .build()
        .unwrap();
    let session = rho.session(SessionOptions::new()).await.unwrap();
    let mut run = session.start(UserInput::text("go")).await.unwrap();

    while !matches!(run.next_event().await, Some(RunEvent::UsageUpdated { .. })) {}
    run.cancel();
    assert!(matches!(run.outcome().await, Err(crate::Error::Cancelled)));

    let events = recorder.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome(), ProviderRequestOutcome::Cancelled);
    assert_eq!(events[0].usage(), &usage);
}

#[tokio::test]
async fn records_each_invalid_response_attempt_with_request_context() {
    let first_usage = ModelUsage {
        input_tokens: Some(11),
        ..ModelUsage::default()
    };
    let second_usage = ModelUsage {
        output_tokens: Some(7),
        ..ModelUsage::default()
    };
    let provider = ScriptedProvider::new(
        identity(),
        [
            ScriptedTurn::streaming(
                vec![ModelEvent::Usage(first_usage.clone())],
                ModelResponse::Assistant(Vec::new()),
            ),
            ScriptedTurn::streaming(
                vec![ModelEvent::Usage(second_usage.clone())],
                ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
            ),
        ],
    );
    let recorder = CapturingRecorder::default();
    let workspace_dir = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(workspace_dir.path()).unwrap();
    let workspace_path = workspace.root().to_path_buf();
    let session_id = SessionId::from_string("session-for-usage").unwrap();
    let parent_session_id = SessionId::from_string("parent-session").unwrap();
    let rho = Rho::builder()
        .provider(provider)
        .workspace(workspace)
        .usage_parent_session_id(parent_session_id.clone())
        .usage_recorder(recorder.clone())
        .usage_purpose("agent-test")
        .build()
        .unwrap();

    rho.session(SessionOptions::new().id(session_id.clone()))
        .await
        .unwrap()
        .complete("go")
        .await
        .unwrap();

    let events = recorder.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].outcome(), ProviderRequestOutcome::InvalidResponse);
    assert_eq!(events[0].usage(), &first_usage);
    assert_eq!(events[1].outcome(), ProviderRequestOutcome::Completed);
    assert_eq!(events[1].usage(), &second_usage);
    for (index, event) in events.iter().enumerate() {
        assert!(!event.event_id().is_empty());
        assert!(event.timestamp_utc_ms() > 0);
        assert_eq!(event.context().identity(), &identity());
        assert_eq!(event.context().session_id(), Some(&session_id));
        assert_eq!(
            event.context().parent_session_id(),
            Some(&parent_session_id)
        );
        assert!(!event
            .context()
            .run_id()
            .expect("agent request has a run ID")
            .as_str()
            .is_empty());
        assert_eq!(event.context().step_index(), Some(1));
        assert_eq!(event.context().attempt_index(), Some(index + 1));
        assert_eq!(
            event.context().workspace_path(),
            Some(workspace_path.as_path())
        );
        assert_eq!(event.context().purpose(), "agent-test");
    }
}

#[tokio::test]
async fn records_provider_internal_retries_as_distinct_physical_attempts() {
    let final_usage = ModelUsage {
        output_tokens: Some(5),
        ..ModelUsage::default()
    };
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming_with_request_events(
            vec![
                ProviderStreamEvent::Request(ProviderRequestEvent::RequestAttemptFailed {
                    kind: ProviderErrorKind::Unavailable,
                    usage: ModelUsage::default(),
                }),
                ProviderStreamEvent::Model(ModelEvent::Usage(final_usage.clone())),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )],
    );
    let recorder = CapturingRecorder::default();
    let rho = Rho::builder()
        .provider(provider)
        .usage_recorder(recorder.clone())
        .build()
        .unwrap();

    rho.session(SessionOptions::new())
        .await
        .unwrap()
        .complete("go")
        .await
        .unwrap();

    let events = recorder.events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].outcome(),
        ProviderRequestOutcome::Failed(ProviderErrorKind::Unavailable)
    );
    assert_eq!(events[0].usage(), &ModelUsage::default());
    assert_eq!(events[0].context().attempt_index(), Some(1));
    assert_eq!(events[1].outcome(), ProviderRequestOutcome::Completed);
    assert_eq!(events[1].usage(), &final_usage);
    assert_eq!(events[1].context().attempt_index(), Some(2));
}

#[tokio::test]
async fn recorder_failure_is_non_fatal_and_exposed_as_bounded_diagnostic() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let recorder = CapturingRecorder {
        failure: Some(ProviderRequestUsageRecorderError::new("x".repeat(2_000))),
        ..CapturingRecorder::default()
    };
    let rho = Rho::builder()
        .provider(provider)
        .usage_recorder(recorder)
        .build()
        .unwrap();

    let outcome = rho
        .session(SessionOptions::new())
        .await
        .unwrap()
        .complete("go")
        .await
        .unwrap();

    assert_eq!(outcome.text(), "done");
    let diagnostics = rho.diagnostics();
    assert_eq!(diagnostics.usage_recorder_diagnostics().len(), 1);
    assert_eq!(
        diagnostics.usage_recorder_diagnostics()[0].message().len(),
        MAX_DIAGNOSTIC_BYTES
    );
}

#[test]
fn diagnostics_drop_oldest_entries_at_the_bound() {
    let diagnostics = UsageRecorderDiagnostics::default();
    for index in 0..20 {
        diagnostics.push(ProviderRequestUsageRecorderError::new(index.to_string()));
    }
    let entries = diagnostics.snapshot();
    assert_eq!(entries.len(), MAX_DIAGNOSTICS);
    assert_eq!(entries.first().unwrap().message(), "4");
    assert_eq!(entries.last().unwrap().message(), "19");
}
