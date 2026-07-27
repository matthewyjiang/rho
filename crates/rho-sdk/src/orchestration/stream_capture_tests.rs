use crate::{
    model::{ModelEvent, ModelIdentity, ModelUsage},
    RunEvent,
};

use super::{capture_provider_event, StreamCapture};

fn identity() -> ModelIdentity {
    ModelIdentity::new("scripted", "test", "model")
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
