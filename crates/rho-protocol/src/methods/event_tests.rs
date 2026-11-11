use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::{
    display::{
        DiffRow, DiffRowKind, NotificationCard, NotificationDelivery, NotificationPreview,
        NotificationTone, NotificationVisibility, Presentation, ToolBody, ToolCard, ToolCardStatus,
        ToolFact, ToolFamily, ToolHeader,
    },
    methods::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome},
};

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
                turn: Some(TurnId::new("t1")),
            },
            json!({"type": "state", "state": "running", "turn": "t1"}),
        ),
        (
            SessionEvent::State {
                state: JobState::Idle,
                turn: None,
            },
            json!({"type": "state", "state": "idle"}),
        ),
        (
            SessionEvent::TurnEnded {
                turn: TurnId::new("t1"),
                outcome: TurnOutcome::Answered {
                    text: "done".into(),
                    stop: AnswerStop::EndTurn,
                },
            },
            json!({"type": "turn_ended", "turn": "t1", "outcome": {"type": "answered", "text": "done", "stop": "end_turn"}}),
        ),
        (
            SessionEvent::TurnEnded {
                turn: TurnId::new("t1"),
                outcome: TurnOutcome::Failed {
                    error: TurnError {
                        kind: TurnErrorKind::Provider,
                        message: "provider down".into(),
                    },
                },
            },
            json!({"type": "turn_ended", "turn": "t1", "outcome": {"type": "failed", "error": {"kind": "provider", "message": "provider down"}}}),
        ),
        (
            SessionEvent::TurnEnded {
                turn: TurnId::new("t1"),
                outcome: TurnOutcome::Cancelled,
            },
            json!({"type": "turn_ended", "turn": "t1", "outcome": {"type": "cancelled"}}),
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
                steer: None,
            },
            json!({"type": "user_message", "message": "m1", "content": [
                {"type": "text", "text": "hi"},
                {"type": "image", "mime_type": "image/png", "data": "aGk="},
            ]}),
        ),
        (
            SessionEvent::UserMessage {
                message: message(),
                content: vec![ContentBlock::Text {
                    text: "use tabs".into(),
                }],
                steer: Some(SteerId::new("s1")),
            },
            json!({
                "type": "user_message",
                "message": "m1",
                "content": [{"type": "text", "text": "use tabs"}],
                "steer": "s1",
            }),
        ),
        (
            SessionEvent::SteeringDelivered {
                id: SteerId::new("s1"),
            },
            json!({"type": "steering_delivered", "id": "s1"}),
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
                        deleted: false,
                    },
                ]),
                locations: Some(vec![Location {
                    path: "a.rs".into(),
                    line: Some(3),
                }]),
                display: None,
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
            SessionEvent::tool_call(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: None,
                kind: None,
                status: Some(ToolStatus::Completed),
                raw_input: None,
                content: None,
                locations: None,
                display: None,
            }),
            json!({"type": "tool_call", "id": "t1", "status": "completed"}),
        ),
        (
            SessionEvent::tool_call(ToolCallUpdate {
                display: Some(Presentation::Card(
                    ToolCard::new(
                        ToolCardStatus::Running,
                        ToolFamily::FileCommand,
                        ToolHeader::shell("$", Some("cargo test".into())),
                    )
                    .with_facts(vec![ToolFact::Timeout { seconds: Some(30) }])
                    .with_body(ToolBody::Diff(vec![DiffRow::new(
                        DiffRowKind::Added,
                        Some(2),
                        "new",
                    )])),
                )),
                ..status_only(ToolStatus::InProgress)
            }),
            json!({
                "type": "tool_call",
                "id": "t1",
                "status": "in_progress",
                "display": {"card": {
                    "status": "running",
                    "family": "file_command",
                    "header": {"kind": "shell", "prompt": "$", "command": "cargo test"},
                    "facts": [{"kind": "timeout", "seconds": 30}],
                    "body": {"kind": "diff", "lines": [{"kind": "added", "line": 2, "text": "new"}]},
                }},
            }),
        ),
        (
            // A case-sensitive search leaves the flag to its default.
            SessionEvent::tool_call(ToolCallUpdate {
                display: Some(Presentation::Card(
                    ToolCard::new(
                        ToolCardStatus::Ok,
                        ToolFamily::FileCommand,
                        ToolHeader::shell("$", Some("rg needle".into())),
                    )
                    .with_body(ToolBody::Search {
                        pattern: "needle".into(),
                        literal: false,
                        case_sensitive: true,
                        lines: vec!["a.rs:1:needle".into()],
                    }),
                )),
                ..status_only(ToolStatus::Completed)
            }),
            json!({
                "type": "tool_call",
                "id": "t1",
                "status": "completed",
                "display": {"card": {
                    "status": "ok",
                    "family": "file_command",
                    "header": {"kind": "shell", "prompt": "$", "command": "rg needle"},
                    "body": {"kind": "search", "lines": {
                        "pattern": "needle",
                        "lines": ["a.rs:1:needle"],
                    }},
                }},
            }),
        ),
        (
            SessionEvent::tool_call(ToolCallUpdate {
                display: Some(Presentation::Notification(Box::new(NotificationCard {
                    title: "Agent result".into(),
                    sender: "worker".into(),
                    recipient: "agent".into(),
                    delivery: NotificationDelivery::Received,
                    tone: NotificationTone::default(),
                    preview: NotificationPreview::default(),
                    visibility: NotificationVisibility::default(),
                    reference: None,
                    subtitle: None,
                    body: "done".into(),
                    output: None,
                    details: Vec::new(),
                }))),
                ..status_only(ToolStatus::Completed)
            }),
            json!({
                "type": "tool_call",
                "id": "t1",
                "status": "completed",
                "display": {"message": {
                    "title": "Agent result",
                    "sender": "worker",
                    "recipient": "agent",
                    "delivery": "received",
                    "body": "done",
                    "details": [],
                }},
            }),
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
                cost_usd_micros: Some(1_250_000),
            },
            json!({
                "type": "usage",
                "used_tokens": 10,
                "window_tokens": 200,
                "cost_usd_micros": 1_250_000,
            }),
        ),
        (
            SessionEvent::Usage {
                used_tokens: 10,
                window_tokens: None,
                cost_usd_micros: None,
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
                status: CompactionStatus::Completed {
                    counts: CompactionCounts {
                        previous_messages: 12,
                        current_messages: 3,
                        previous_tokens: 9000,
                        current_tokens: 1200,
                        cost_usd_micros: None,
                    },
                },
            },
            json!({
                "type": "compaction",
                "id": "c1",
                "status": {
                    "type": "completed",
                    "counts": {
                        "previous_messages": 12,
                        "current_messages": 3,
                        "previous_tokens": 9000,
                        "current_tokens": 1200,
                    },
                },
            }),
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
            SessionEvent::ProviderRetry {
                reason: RetryReason::RateLimit,
                retry_after_ms: Some(5000),
            },
            json!({"type": "provider_retry", "reason": "rate_limit", "retry_after_ms": 5000}),
        ),
        (
            SessionEvent::ProviderRetry {
                reason: RetryReason::RequestFailed,
                retry_after_ms: None,
            },
            json!({"type": "provider_retry", "reason": "request_failed"}),
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
            SessionEvent::tool_call(ToolCallUpdate {
                id: ToolCallId::new("t1"),
                title: None,
                kind: Some(ToolKind::Unknown),
                status: Some(ToolStatus::Unknown),
                raw_input: None,
                content: Some(vec![ToolContent::Unknown]),
                locations: None,
                display: None,
            }),
        ),
        (
            json!({
                "type": "tool_call",
                "id": "t1",
                "status": "paused",
                "display": {"card": {
                    "status": "paused",
                    "family": "hologram",
                    "header": {"kind": "banner", "text": "hi"},
                    "facts": [{"kind": "gauge", "value": 3}],
                }},
            }),
            SessionEvent::tool_call(ToolCallUpdate {
                display: Some(Presentation::Card(
                    ToolCard::new(
                        ToolCardStatus::Unknown,
                        ToolFamily::Unknown,
                        ToolHeader::Unknown,
                    )
                    .with_facts(vec![ToolFact::Unknown]),
                )),
                ..status_only(ToolStatus::Unknown)
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

/// Tool call `t1` moving to `status`, with every other field absent.
fn status_only(status: ToolStatus) -> ToolCallUpdate {
    ToolCallUpdate {
        id: ToolCallId::new("t1"),
        title: None,
        kind: None,
        status: Some(status),
        raw_input: None,
        content: None,
        locations: None,
        display: None,
    }
}
