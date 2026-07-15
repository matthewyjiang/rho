use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage,
        ProviderContextBlock, ToolSpec,
    },
    CancellationToken, ProviderErrorKind,
};

use super::{provider_event_channel, ModelProvider, ScriptedProvider, ScriptedTurn};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

fn request<'a>(
    messages: &'a [Message],
    tools: &'a [ToolSpec],
    cancellation: CancellationToken,
) -> ModelRequest<'a> {
    ModelRequest {
        messages,
        tools,
        cancellation,
        reasoning_level: crate::ReasoningLevel::default(),
        prompt_cache_key: Some("session-1"),
    }
}

#[tokio::test]
async fn trait_object_completes_text_and_records_provider_neutral_request() {
    let provider: Box<dyn ModelProvider> = Box::new(ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("hello".into()),
        ]))],
    ));
    let messages = [Message::user_text("hi")];
    let tools = [ToolSpec {
        name: "lookup".into(),
        description: "look up a value".into(),
        input_schema: json!({"type": "object"}),
    }];

    let response = provider
        .send_turn(request(&messages, &tools, CancellationToken::new()))
        .await
        .unwrap();

    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())])
    );
}

#[tokio::test]
async fn streaming_preserves_event_order_usage_and_native_replay_context() {
    let usage = ModelUsage {
        output_tokens: Some(2),
        ..ModelUsage::default()
    };
    let context = ProviderContextBlock {
        identity: identity(),
        kind: "test_context".into(),
        position: Some(0),
        data: json!({"opaque": "value"}),
    };
    let expected = vec![
        ModelEvent::OutputDelta("he".into()),
        ModelEvent::OutputDelta("llo".into()),
        ModelEvent::ProviderContext {
            kind: context.kind.clone(),
            position: context.position,
            data: context.data.clone(),
        },
        ModelEvent::Usage(usage),
    ];
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            expected.clone(),
            ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())]),
        )],
    );
    let (events, mut receiver) = provider_event_channel(NonZeroUsize::new(1).unwrap());
    let messages = [Message::user_text("hi")];
    let cancellation = CancellationToken::new();

    let (result, received) = tokio::join!(
        provider.send_turn_stream(request(&messages, &[], cancellation), events),
        async {
            let mut received = Vec::new();
            while let Some(event) = receiver.recv().await {
                received.push(event);
                if received.len() == expected.len() {
                    break;
                }
            }
            received
        }
    );

    assert_eq!(
        result.unwrap(),
        ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())])
    );
    assert_eq!(received, expected);
}

#[tokio::test]
async fn cancellation_stops_a_scripted_request_before_consuming_a_turn() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![]))],
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = provider
        .send_turn(request(&[], &[], cancellation))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::Interrupted);
    assert!(provider.recorded_requests().is_empty());
}

#[tokio::test]
async fn dropping_the_event_consumer_interrupts_streaming_deterministically() {
    let provider = ScriptedProvider::new(
        identity(),
        [ScriptedTurn::streaming(
            vec![ModelEvent::OutputDelta("hello".into())],
            ModelResponse::Assistant(vec![ContentBlock::Text("hello".into())]),
        )],
    );
    let (events, receiver) = provider_event_channel(NonZeroUsize::new(1).unwrap());
    drop(receiver);

    let error = provider
        .send_turn_stream(request(&[], &[], CancellationToken::new()), events)
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::Interrupted);
}

#[tokio::test]
async fn exhausted_script_is_reported_as_invalid_response() {
    let provider = ScriptedProvider::new(identity(), []);

    let error = provider
        .send_turn(request(&[], &[], CancellationToken::new()))
        .await
        .unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::InvalidResponse);
}
