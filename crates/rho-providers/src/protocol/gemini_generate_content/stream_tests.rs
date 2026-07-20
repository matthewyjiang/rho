use super::*;
use crate::model::{ContentBlock, ModelEvent};

#[test]
fn decoder_joins_multiline_data_and_ignores_comments_and_done() {
    let mut decoder = SseEventDecoder::default();
    let mut collector = ResponseCollector::default();
    let mut events = Vec::new();
    let mut on_event = |event| {
        events.push(event);
        Ok(())
    };

    assert!(!decoder
        .apply_line(": keep-alive", &mut collector, &mut on_event)
        .unwrap());
    for line in [
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},",
        "data: \"finishReason\":\"STOP\"}]}",
        "",
        "data: [DONE]",
        "",
    ] {
        decoder
            .apply_line(line, &mut collector, &mut on_event)
            .unwrap();
    }

    assert_eq!(
        collector.finish().unwrap(),
        ModelResponse::Assistant(vec![ContentBlock::Text("ok".into())])
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, ModelEvent::OutputDelta(text) if text == "ok")));
}

#[test]
fn stream_errors_after_output_are_not_retryable_as_fresh_requests() {
    let mut collector = ResponseCollector::default();
    let response: GenerateContentResponse = serde_json::from_value(serde_json::json!({
        "candidates": [{"content":{"parts":[{"text":"partial"}]}}]
    }))
    .unwrap();
    collector.apply(response, None).unwrap();

    let error = stream_error(
        &collector,
        ModelError::InvalidResponse("broken event".into()),
    );

    assert!(matches!(
        error,
        ModelError::StreamFailedAfterOutput { message } if message.contains("broken event")
    ));
}

#[test]
fn decoder_rejects_malformed_event_when_delimited() {
    let mut decoder = SseEventDecoder::default();
    let mut collector = ResponseCollector::default();

    decoder
        .apply_line("data: {", &mut collector, &mut |_| Ok(()))
        .unwrap();
    let error = decoder
        .apply_line("", &mut collector, &mut |_| Ok(()))
        .unwrap_err();

    assert!(
        matches!(error, ModelError::InvalidResponse(message) if message.contains("invalid Gemini stream event"))
    );
}
