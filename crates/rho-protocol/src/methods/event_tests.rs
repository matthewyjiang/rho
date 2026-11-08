use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::methods::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome};

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
                state: JobState::Running,
            },
            json!({"type": "state", "state": "running"}),
        ),
        (
            SessionEvent::TurnEnded {
                outcome: TurnOutcome::Answered {
                    text: "done".into(),
                    stop: AnswerStop::EndTurn,
                },
            },
            json!({"type": "turn_ended", "outcome": {"type": "answered", "text": "done", "stop": "end_turn"}}),
        ),
        (
            SessionEvent::TurnEnded {
                outcome: TurnOutcome::Failed {
                    error: TurnError {
                        kind: TurnErrorKind::Provider,
                        message: "provider down".into(),
                    },
                },
            },
            json!({"type": "turn_ended", "outcome": {"type": "failed", "error": {"kind": "provider", "message": "provider down"}}}),
        ),
        (
            SessionEvent::TurnEnded {
                outcome: TurnOutcome::Cancelled,
            },
            json!({"type": "turn_ended", "outcome": {"type": "cancelled"}}),
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
            SessionEvent::UserMessage {
                message: message(),
                content: vec![
                    ContentBlock::Resource {
                        uri: "file:///w/a.rs".into(),
                        mime_type: Some("text/x-rust".into()),
                        text: "fn a() {}".into(),
                    },
                    ContentBlock::Resource {
                        uri: "file:///w/b.txt".into(),
                        mime_type: None,
                        text: "b".into(),
                    },
                    ContentBlock::ResourceLink {
                        uri: "file:///w/notes.md".into(),
                        name: "notes.md".into(),
                        title: Some("Notes".into()),
                        description: Some("meeting notes".into()),
                        mime_type: Some("text/markdown".into()),
                    },
                    ContentBlock::ResourceLink {
                        uri: "file:///w/c".into(),
                        name: "c".into(),
                        title: None,
                        description: None,
                        mime_type: None,
                    },
                ],
            },
            json!({"type": "user_message", "message": "m1", "content": [
                {"type": "resource", "uri": "file:///w/a.rs", "mime_type": "text/x-rust", "text": "fn a() {}"},
                {"type": "resource", "uri": "file:///w/b.txt", "text": "b"},
                {
                    "type": "resource_link",
                    "uri": "file:///w/notes.md",
                    "name": "notes.md",
                    "title": "Notes",
                    "description": "meeting notes",
                    "mime_type": "text/markdown",
                },
                {"type": "resource_link", "uri": "file:///w/c", "name": "c"},
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
            SessionEvent::tool_call(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: Some("Edit a.rs".into()),
                kind: Some(ToolKind::SwitchMode),
                status: Some(ToolStatus::InProgress),
                raw_input: Some(json!({"path": "a.rs"})),
                raw_output: Some(json!("ok")),
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
                "raw_output": "ok",
                "content": [
                    {"type": "text", "text": "ok"},
                    {"type": "image", "mime_type": "image/png", "data": "aGk="},
                    {"type": "diff", "path": "a.rs", "new_text": "fn a() {}"},
                ],
                "locations": [{"path": "a.rs", "line": 3}],
            }),
        ),
        (
            SessionEvent::tool_call(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: None,
                kind: None,
                status: Some(ToolStatus::Completed),
                raw_input: None,
                raw_output: None,
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
                cost: Some(Cost {
                    amount_micros: 1_250_000,
                    currency: "USD".into(),
                }),
            },
            json!({
                "type": "usage",
                "used_tokens": 10,
                "window_tokens": 200,
                "cost": {"amount_micros": 1_250_000, "currency": "USD"},
            }),
        ),
        (
            SessionEvent::Usage {
                used_tokens: 10,
                window_tokens: None,
                cost: None,
            },
            json!({"type": "usage", "used_tokens": 10}),
        ),
        (
            SessionEvent::TitleChanged {
                title: "Fix the parser".into(),
            },
            json!({"type": "title_changed", "title": "Fix the parser"}),
        ),
        (
            SessionEvent::Compaction {
                id: CompactionId::new("c1"),
                status: CompactionStatus::Started,
            },
            json!({"type": "compaction", "id": "c1", "status": {"type": "started"}}),
        ),
        (
            SessionEvent::Compaction {
                id: CompactionId::new("c1"),
                status: CompactionStatus::Completed,
            },
            json!({"type": "compaction", "id": "c1", "status": {"type": "completed"}}),
        ),
        (
            SessionEvent::Compaction {
                id: CompactionId::new("c1"),
                status: CompactionStatus::Cancelled,
            },
            json!({"type": "compaction", "id": "c1", "status": {"type": "cancelled"}}),
        ),
        (
            SessionEvent::Compaction {
                id: CompactionId::new("c1"),
                status: CompactionStatus::Failed {
                    error: "provider down".into(),
                },
            },
            json!({
                "type": "compaction",
                "id": "c1",
                "status": {"type": "failed", "error": "provider down"},
            }),
        ),
        (
            SessionEvent::ResponseDiscarded,
            json!({"type": "response_discarded"}),
        ),
        (
            SessionEvent::Card {
                title: "Job done".into(),
                markdown: "**ok**".into(),
                plain: "ok".into(),
                tone: CardTone::Error,
            },
            json!({
                "type": "card",
                "title": "Job done",
                "markdown": "**ok**",
                "plain": "ok",
                "tone": "error",
            }),
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

// Covers: an event type, tool kind, tool status, tool content, compaction
// status, card tone, or content block a newer daemon adds reads as
// `Unknown` instead of failing the notification.
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
            SessionEvent::tool_call(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: None,
                kind: Some(ToolKind::Unknown),
                status: Some(ToolStatus::Unknown),
                raw_input: None,
                raw_output: None,
                content: Some(vec![ToolContent::Unknown]),
                locations: None,
            }),
        ),
        (
            json!({"type": "compaction", "id": "c1", "status": {"type": "paused"}}),
            SessionEvent::Compaction {
                id: CompactionId::new("c1"),
                status: CompactionStatus::Unknown,
            },
        ),
        (
            json!({"type": "card", "title": "t", "markdown": "m", "plain": "p", "tone": "loud"}),
            SessionEvent::Card {
                title: "t".into(),
                markdown: "m".into(),
                plain: "p".into(),
                tone: CardTone::Unknown,
            },
        ),
        (
            json!({"type": "user_message", "message": "m1", "content": [{"type": "audio"}]}),
            SessionEvent::UserMessage {
                message: MessageId::new("m1"),
                content: vec![ContentBlock::Unknown],
            },
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
