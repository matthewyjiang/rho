use pretty_assertions::assert_eq;
use rho_sdk::model::ModelStopReason;

use super::*;

// Covers: Anthropic's message_delta stop_reason reaches the SDK as a typed stop
// Owner: anthropic messages streaming
#[test]
fn message_delta_stop_reason_reports_early_stops() {
    for (stop_reason, expected) in [
        ("end_turn", None),
        ("tool_use", None),
        ("max_tokens", Some(ModelStopReason::MaxTokens)),
        (
            "model_context_window_exceeded",
            Some(ModelStopReason::MaxTokens),
        ),
        ("refusal", Some(ModelStopReason::Refusal)),
    ] {
        let mut stops = Vec::new();
        let line = format!(
            r#"data: {{"type":"message_delta","delta":{{"stop_reason":"{stop_reason}"}},"usage":{{"output_tokens":3}}}}"#
        );
        handle_anthropic_stream_line(&line, &mut AnthropicSseState::default(), &mut |event| {
            stops.extend(event.as_stop_reason());
            Ok(())
        })
        .unwrap();

        assert_eq!(stops, Vec::from_iter(expected), "{stop_reason}");
    }
}
