use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_sdk::{
    tool::ToolProgress, ProviderStreamResetReason, Revision, RunEvent, RunId, ToolCallId,
    ToolCompletion,
};
use serde_json::json;

use super::{parse_duration, JsonlAdapter};

#[test]
fn parses_human_durations_and_rejects_invalid_values() {
    assert_eq!(parse_duration("20m").unwrap(), Duration::from_secs(1_200));
    assert!(parse_duration("soon")
        .unwrap_err()
        .contains("invalid duration"));
}

#[test]
fn serializes_started_event_with_version_and_monotonic_sequence() {
    let mut adapter = JsonlAdapter::new();
    adapter.set_run_context("session-1", std::path::Path::new("/workspace"));
    let run_id = RunId::from_string("run-1").unwrap();

    let started = adapter
        .event(&RunEvent::Started {
            run_id,
            revision: Revision::from_u64(0),
        })
        .unwrap();
    let delta = adapter
        .event(&RunEvent::AssistantTextDelta { text: "hi".into() })
        .unwrap();

    assert_eq!(
        serde_json::to_value(started).unwrap(),
        json!({
            "schema_version": 1,
            "seq": 1,
            "type": "run.started",
            "run_id": "run-1",
            "session_id": "session-1",
            "workspace": "/workspace"
        })
    );
    assert_eq!(
        serde_json::to_value(delta).unwrap(),
        json!({
            "schema_version": 1,
            "seq": 2,
            "type": "assistant.text_delta",
            "attempt": 1,
            "text": "hi"
        })
    );
}

#[test]
fn emits_safe_tool_lifecycle_fields_only() {
    let mut adapter = JsonlAdapter::new();
    let call_id = ToolCallId::from_string("call-1").unwrap();

    assert!(adapter
        .event(&RunEvent::ToolUpdated {
            call_id: call_id.clone(),
            progress: ToolProgress::message("private progress text"),
        })
        .is_none());
    let updated = adapter
        .event(&RunEvent::ToolUpdated {
            call_id: call_id.clone(),
            progress: ToolProgress::message("private progress text").units(1, 2),
        })
        .unwrap();
    let finished = adapter
        .event(&RunEvent::ToolFinished {
            call_id,
            result: ToolCompletion::Unavailable,
        })
        .unwrap();

    assert_eq!(
        serde_json::to_value(updated).unwrap(),
        json!({
            "schema_version": 1,
            "seq": 1,
            "type": "tool.updated",
            "call_id": "call-1",
            "completed_units": 1,
            "total_units": 2
        })
    );
    assert_eq!(
        serde_json::to_value(finished).unwrap(),
        json!({
            "schema_version": 1,
            "seq": 2,
            "type": "tool.finished",
            "call_id": "call-1",
            "status": "unavailable"
        })
    );
}

#[test]
fn reset_closes_the_old_attempt_and_advances_new_deltas() {
    let mut adapter = JsonlAdapter::new();
    let first = adapter
        .event(&RunEvent::AssistantTextDelta {
            text: "discarded".into(),
        })
        .unwrap();
    let reset = adapter
        .event(&RunEvent::ProviderStreamReset {
            reason: ProviderStreamResetReason::InvalidResponse,
            detail: "private provider detail".into(),
        })
        .unwrap();
    let second = adapter
        .event(&RunEvent::AssistantTextDelta {
            text: "kept".into(),
        })
        .unwrap();

    assert_eq!(serde_json::to_value(first).unwrap()["attempt"], 1);
    assert_eq!(
        serde_json::to_value(reset).unwrap(),
        json!({
            "schema_version": 1,
            "seq": 2,
            "type": "assistant.text_reset",
            "attempt": 1
        })
    );
    assert_eq!(serde_json::to_value(second).unwrap()["attempt"], 2);
    assert_eq!(adapter.partial_text().as_deref(), Some("kept"));
}
