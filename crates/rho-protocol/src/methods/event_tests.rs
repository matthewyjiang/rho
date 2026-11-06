use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: every event's wire form (the `type` tag, snake_case names, and
// absent optionals left out) round-trips, which an ACP adapter or another
// client maps by name.
// Owner: protocol wire contract.
#[test]
fn every_session_event_round_trips_through_its_wire_form() {
    let message = || MessageId::new("m1");
    let cases = [
        (
            SessionEvent::State {
                state: JobState::Failed,
                error: Some("provider down".into()),
            },
            json!({"type": "state", "state": "failed", "error": "provider down"}),
        ),
        (
            SessionEvent::State {
                state: JobState::Running,
                error: None,
            },
            json!({"type": "state", "state": "running"}),
        ),
        (
            SessionEvent::UserMessage {
                message: message(),
                content: vec![
                    ContentBlock::Text { text: "hi".into() },
                    ContentBlock::Image {
                        mime_type: "image/png".into(),
                        data: "aGk=".into(),
                    },
                ],
            },
            json!({"type": "user_message", "message": "m1", "content": [
                {"type": "text", "text": "hi"},
                {"type": "image", "mime_type": "image/png", "data": "aGk="},
            ]}),
        ),
        (
            SessionEvent::AgentMessageChunk {
                message: message(),
                text: "he".into(),
            },
            json!({"type": "agent_message_chunk", "message": "m1", "text": "he"}),
        ),
        (
            SessionEvent::ThoughtChunk {
                message: message(),
                text: "hm".into(),
            },
            json!({"type": "thought_chunk", "message": "m1", "text": "hm"}),
        ),
        (
            SessionEvent::ToolCall(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: Some("Edit a.rs".into()),
                kind: Some(ToolKind::SwitchMode),
                status: Some(ToolStatus::InProgress),
                raw_input: Some(json!({"path": "a.rs"})),
                content: Some(vec![
                    ToolContent::Text { text: "ok".into() },
                    ToolContent::Image {
                        mime_type: "image/png".into(),
                        data: "aGk=".into(),
                    },
                    ToolContent::Diff {
                        path: "a.rs".into(),
                        old_text: None,
                        new_text: "fn a() {}".into(),
                    },
                ]),
                locations: Some(vec![Location {
                    path: "a.rs".into(),
                    line: Some(3),
                }]),
            }),
            json!({
                "type": "tool_call",
                "id": "t1",
                "title": "Edit a.rs",
                "kind": "switch_mode",
                "status": "in_progress",
                "raw_input": {"path": "a.rs"},
                "content": [
                    {"type": "text", "text": "ok"},
                    {"type": "image", "mime_type": "image/png", "data": "aGk="},
                    {"type": "diff", "path": "a.rs", "new_text": "fn a() {}"},
                ],
                "locations": [{"path": "a.rs", "line": 3}],
            }),
        ),
        (
            SessionEvent::ToolCall(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: None,
                kind: None,
                status: Some(ToolStatus::Completed),
                raw_input: None,
                content: None,
                locations: None,
            }),
            json!({"type": "tool_call", "id": "t1", "status": "completed"}),
        ),
        (
            SessionEvent::Plan {
                entries: vec![PlanEntry {
                    content: "read".into(),
                    status: PlanStatus::InProgress,
                }],
            },
            json!({"type": "plan", "entries": [{"content": "read", "status": "in_progress"}]}),
        ),
        (
            SessionEvent::Usage {
                used_tokens: 10,
                window_tokens: Some(200),
            },
            json!({"type": "usage", "used_tokens": 10, "window_tokens": 200}),
        ),
        (
            SessionEvent::Card {
                title: "Job done".into(),
                markdown: "**ok**".into(),
            },
            json!({"type": "card", "title": "Job done", "markdown": "**ok**"}),
        ),
        (
            SessionEvent::Notice {
                text: "saved".into(),
            },
            json!({"type": "notice", "text": "saved"}),
        ),
    ];
    for (event, wire) in cases {
        assert_eq!(serde_json::to_value(&event).unwrap(), wire, "{event:?}");
        assert_eq!(serde_json::from_value::<SessionEvent>(wire).unwrap(), event);
    }
}

// Covers: an event type, tool kind, tool status, or tool content a newer
// daemon adds reads as `Unknown` instead of failing the notification.
// Owner: protocol forward compatibility.
#[test]
fn unknown_event_values_read_as_unknown() {
    let cases = [
        (
            json!({"type": "approval", "id": "a1"}),
            SessionEvent::Unknown,
        ),
        (
            json!({
                "type": "tool_call",
                "id": "t1",
                "kind": "teleport",
                "status": "paused",
                "content": [{"type": "terminal", "id": "x"}],
            }),
            SessionEvent::ToolCall(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: None,
                kind: Some(ToolKind::Unknown),
                status: Some(ToolStatus::Unknown),
                raw_input: None,
                content: Some(vec![ToolContent::Unknown]),
                locations: None,
            }),
        ),
    ];
    for (wire, event) in cases {
        assert_eq!(
            serde_json::from_value::<SessionEvent>(wire.clone()).unwrap(),
            event,
            "{wire}"
        );
    }
}
