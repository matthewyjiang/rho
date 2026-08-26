use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;

use crate::{
    model::{
        AbortedAssistant, GenerationOutputTokens, ModelEvent, ModelIdentity, ModelResponse,
        ModelUsage, ProviderContextBlock,
    },
    provider::{provider_event_channel, ProviderEventReceiver, ProviderFuture},
};

use super::{
    drain_cancelled_provider_events, drain_cooperative_provider_on_cancellation, StreamCapture,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("test-provider", "test-api", "test-model")
}

async fn receiver_with_queued_events() -> ProviderEventReceiver {
    let (events, receiver) = provider_event_channel(NonZeroUsize::new(3).unwrap());
    events
        .send(ModelEvent::GenerationOutputTokens(
            GenerationOutputTokens::Unavailable,
        ))
        .await
        .unwrap();
    events
        .send(ModelEvent::ProviderContext {
            kind: "replayable".into(),
            position: Some(1),
            data: serde_json::json!({ "value": true }),
        })
        .await
        .unwrap();
    events
        .send(ModelEvent::Usage(ModelUsage {
            output_tokens: Some(5),
            ..ModelUsage::default()
        }))
        .await
        .unwrap();
    receiver
}

fn expected_capture() -> AbortedAssistant {
    AbortedAssistant {
        provider_context: vec![ProviderContextBlock {
            identity: identity(),
            kind: "replayable".into(),
            position: Some(1),
            data: serde_json::json!({ "value": true }),
        }],
        usage: ModelUsage {
            output_tokens: Some(5),
            ..ModelUsage::default()
        },
        ..AbortedAssistant::default()
    }
}

// Covers: external cancellation must not retain performance metadata as replay context.
// Owner: SDK provider cancellation drain
#[tokio::test]
async fn external_cancellation_discards_metric_and_keeps_adjacent_events() {
    let mut receiver = receiver_with_queued_events().await;
    let mut capture = StreamCapture::default();

    drain_cancelled_provider_events(&mut receiver, &identity(), &mut capture);

    assert_eq!(capture.into_aborted_assistant(), Some(expected_capture()));
}

// Covers: cooperative cancellation must not retain performance metadata as replay context.
// Owner: SDK provider cancellation drain
#[tokio::test]
async fn cooperative_cancellation_discards_metric_and_keeps_adjacent_events() {
    let mut receiver = receiver_with_queued_events().await;
    let mut capture = StreamCapture::default();
    let mut future: ProviderFuture<'_> =
        Box::pin(async { Ok(ModelResponse::Assistant(Vec::new())) });

    drain_cooperative_provider_on_cancellation(
        &mut future,
        &mut receiver,
        &identity(),
        &mut capture,
    )
    .await;

    assert_eq!(capture.into_aborted_assistant(), Some(expected_capture()));
}
