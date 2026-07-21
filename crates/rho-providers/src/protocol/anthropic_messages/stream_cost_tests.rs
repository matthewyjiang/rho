use pretty_assertions::assert_eq;

use super::{handle_anthropic_stream_line, AnthropicSseState};
use crate::model::{ModelEvent, ModelUsage};

#[test]
fn message_delta_reports_gateway_cost_with_usage() {
    let mut state = AnthropicSseState::default();
    let mut events = Vec::new();

    handle_anthropic_stream_line(
        r#"data: {"type":"message_delta","usage":{"output_tokens":5},"provider_metadata":{"gateway":{"cost":"0.0042"}}}"#,
        &mut state,
        &mut |event| {
            events.push(event);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        events,
        vec![ModelEvent::Usage(ModelUsage {
            output_tokens: Some(5),
            total_tokens: Some(5),
            cost_usd_micros: Some(4_200),
            ..ModelUsage::default()
        })]
    );
}

#[test]
fn message_deltas_emit_only_monotonic_token_and_cost_deltas() {
    let mut state = AnthropicSseState::default();
    let mut events = Vec::new();
    let lines = [
        r#"data: {"type":"message_delta","usage":{"input_tokens":100,"cache_read_input_tokens":20,"output_tokens":5},"provider_metadata":{"gateway":{"cost":0.001}}}"#,
        r#"data: {"type":"message_delta","usage":{"input_tokens":100,"cache_read_input_tokens":20},"provider_metadata":{"gateway":{"cost":0.0015}}}"#,
        r#"data: {"type":"message_delta","usage":{"output_tokens":4},"provider_metadata":{"gateway":{"cost":0.0014}}}"#,
        r#"data: {"type":"message_delta","usage":{"output_tokens":10},"provider_metadata":{"gateway":{"cost":0.002}}}"#,
    ];

    for line in lines {
        handle_anthropic_stream_line(line, &mut state, &mut |event| {
            events.push(event);
            Ok(())
        })
        .unwrap();
    }

    let usage = events
        .into_iter()
        .filter_map(|event| match event {
            ModelEvent::Usage(usage) => Some(usage),
            _ => None,
        })
        .fold(ModelUsage::default(), |total, usage| {
            total.saturating_add(&usage)
        });
    assert_eq!(
        usage,
        ModelUsage {
            output_tokens: Some(10),
            total_tokens: Some(10),
            cost_usd_micros: Some(2_000),
            ..ModelUsage::default()
        }
    );
}

#[test]
fn message_delta_reports_gateway_cost_without_token_usage() {
    let mut state = AnthropicSseState::default();
    let mut events = Vec::new();

    handle_anthropic_stream_line(
        r#"data: {"type":"message_delta","provider_metadata":{"gateway":{"cost":0}}}"#,
        &mut state,
        &mut |event| {
            events.push(event);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        events,
        vec![ModelEvent::Usage(ModelUsage {
            cost_usd_micros: Some(0),
            ..ModelUsage::default()
        })]
    );
}
