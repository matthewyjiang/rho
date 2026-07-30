use crate::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelUsage, PartialToolCall, ToolCall},
    RunEvent,
};

use super::{capture_provider_event, StreamCapture};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
}

fn capture_tool_delta(
    capture: &mut StreamCapture,
    index: usize,
    id: Option<&str>,
    name: Option<&str>,
    arguments: &str,
) -> RunEvent {
    capture_provider_event(
        ModelEvent::ToolCallDelta {
            index,
            id: id.map(str::to_owned),
            name: name.map(str::to_owned),
            arguments: arguments.to_owned(),
        },
        &identity(),
        &ModelUsage::default(),
        capture,
    )
}

#[test]
fn tool_call_updates_reemit_known_identity_on_later_argument_deltas() {
    let mut capture = StreamCapture::default();
    let usage = ModelUsage::default();
    let identity = identity();

    // Providers announce identity before arguments, then stream bare deltas.
    let first = capture_provider_event(
        ModelEvent::ToolCallDelta {
            index: 0,
            id: Some("call-1".into()),
            name: Some("read_file".into()),
            arguments: String::new(),
        },
        &identity,
        &usage,
        &mut capture,
    );
    assert!(
        matches!(
            first,
            RunEvent::ToolCallUpdated {
                index: 0,
                id: Some(ref id),
                name: Some(ref name),
                ref arguments_delta,
            } if id == "call-1" && name == "read_file" && arguments_delta.is_empty()
        ),
        "{first:?}"
    );

    let second = capture_provider_event(
        ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: r#"{"path":"src/main.rs"}"#.into(),
        },
        &identity,
        &usage,
        &mut capture,
    );
    assert!(
        matches!(
            second,
            RunEvent::ToolCallUpdated {
                index: 0,
                id: Some(ref id),
                name: Some(ref name),
                ref arguments_delta,
            } if id == "call-1"
                && name == "read_file"
                && arguments_delta == r#"{"path":"src/main.rs"}"#
        ),
        "{second:?}"
    );
}

#[test]
fn tool_call_arguments_complete_across_nested_and_escaped_fragments() {
    let arguments = r#" {"path":"a\\\"b","nested":[{"brace":"}"}],"enabled":true} "#;
    let mut capture = StreamCapture::default();

    for character in arguments.chars() {
        capture_tool_delta(&mut capture, 2, None, None, &character.to_string());
    }
    capture_tool_delta(&mut capture, 2, Some("call-2"), None, "");
    capture_tool_delta(&mut capture, 2, None, Some("write_file"), "");

    let aborted = capture.into_aborted_assistant().unwrap();
    assert_eq!(
        aborted.content,
        vec![ContentBlock::ToolCall(ToolCall {
            id: "call-2".into(),
            name: "write_file".into(),
            arguments: serde_json::from_str(arguments).unwrap(),
        })]
    );
    assert_eq!(
        aborted.tool_calls,
        vec![PartialToolCall {
            id: Some("call-2".into()),
            name: Some("write_file".into()),
            arguments: arguments.into(),
        }]
    );
}

#[test]
fn incomplete_and_non_object_arguments_remain_partial_only() {
    for arguments in [r#"{"path":"incomplete""#, r#"[{"path":"array"}]"#] {
        let mut capture = StreamCapture::default();
        capture_tool_delta(
            &mut capture,
            0,
            Some("call-1"),
            Some("read_file"),
            arguments,
        );

        let aborted = capture.into_aborted_assistant().unwrap();
        assert!(aborted.content.is_empty());
        assert_eq!(
            aborted.tool_calls,
            vec![PartialToolCall {
                id: Some("call-1".into()),
                name: Some("read_file".into()),
                arguments: arguments.into(),
            }]
        );
    }
}

#[test]
fn multi_chunk_object_arguments_materialize_on_aborted_capture() {
    // Awkward chunk width keeps the completion detector crossing token boundaries.
    const CHUNK_BYTES: usize = 17;
    let prefix = r#"{"data":""#;
    let suffix = r#""}"#;
    let payload_len = 2 * 1024;
    let mut arguments = String::with_capacity(prefix.len() + payload_len + suffix.len());
    arguments.push_str(prefix);
    arguments.extend(std::iter::repeat_n('x', payload_len));
    arguments.push_str(suffix);

    let mut capture = StreamCapture::default();
    let mut offset = 0usize;
    let mut first = true;
    while offset < arguments.len() {
        let end = (offset + CHUNK_BYTES).min(arguments.len());
        let id = first.then_some("call-large");
        let name = first.then_some("write_file");
        capture_tool_delta(&mut capture, 0, id, name, &arguments[offset..end]);
        first = false;
        offset = end;
    }

    let aborted = capture.into_aborted_assistant().unwrap();
    assert_eq!(
        aborted.content,
        vec![ContentBlock::ToolCall(ToolCall {
            id: "call-large".into(),
            name: "write_file".into(),
            arguments: serde_json::from_str(&arguments).unwrap(),
        })]
    );
    assert_eq!(
        aborted.tool_calls,
        vec![PartialToolCall {
            id: Some("call-large".into()),
            name: Some("write_file".into()),
            arguments,
        }]
    );
}

#[test]
fn hosted_tool_activity_maps_to_named_run_event() {
    let mut capture = StreamCapture::default();
    let event = capture_provider_event(
        ModelEvent::hosted_tool_activity("x_search", "for \"xAI\""),
        &identity(),
        &ModelUsage::default(),
        &mut capture,
    );
    assert_eq!(
        event,
        RunEvent::HostedToolActivity {
            name: "x_search".into(),
            detail: "for \"xAI\"".into(),
        }
    );
}

#[test]
fn web_search_activity_keeps_stable_run_event_shape() {
    let mut capture = StreamCapture::default();
    let event = capture_provider_event(
        ModelEvent::WebSearch("for \"rho\"".into()),
        &identity(),
        &ModelUsage::default(),
        &mut capture,
    );
    assert_eq!(
        event,
        RunEvent::WebSearch {
            detail: "for \"rho\"".into(),
        }
    );
}

// Covers: service-tier fallback must lower to a typed run event without replay state.
// Owner: SDK stream capture
#[test]
fn service_tier_fallback_maps_to_typed_run_event() {
    let mut capture = StreamCapture::default();
    let event = capture_provider_event(
        ModelEvent::service_tier_fallback(crate::model::ServiceTier::Priority, "default"),
        &identity(),
        &ModelUsage::default(),
        &mut capture,
    );
    assert_eq!(
        event,
        RunEvent::ProviderServiceTierFallback {
            requested: crate::model::ServiceTier::Priority,
            used: "default".into(),
        }
    );
    assert!(capture.provider_context.is_empty());
}
