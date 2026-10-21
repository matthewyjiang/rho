use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ImageContent, MessageId, SessionId, SessionNotification,
    SessionUpdate, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields,
};
use pretty_assertions::assert_eq;
use rho_sdk::model::{self, AbortedAssistant, AssistantMessage, Message, ToolResult};
use serde_json::json;

use super::history;

fn call_block(id: &str) -> model::ContentBlock {
    model::ContentBlock::ToolCall(model::ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: json!({"path": "a.png"}),
    })
}

fn raw_replay(messages: &[Message]) -> Vec<SessionNotification> {
    history(
        &SessionId::new("s1"),
        messages,
        std::path::Path::new(""),
        &Default::default(),
    )
}

/// Replay with ACP identities canonicalized by first appearance: tool calls
/// as `0`, `1`, ... and messages as `m0`, `m1`, ... Pairing is retained, and
/// duplicate announcements or updates to unknown calls are rejected.
fn replay(messages: &[Message]) -> Vec<SessionNotification> {
    let mut notifications = raw_replay(messages);
    let mut ids = std::collections::HashMap::new();
    let mut message_ids = std::collections::HashMap::new();
    for notification in &mut notifications {
        let id = match &mut notification.update {
            SessionUpdate::ToolCall(call) => {
                let next = ids.len().to_string();
                assert!(ids.insert(call.tool_call_id.clone(), next).is_none());
                &mut call.tool_call_id
            }
            SessionUpdate::ToolCallUpdate(update) => &mut update.tool_call_id,
            SessionUpdate::UserMessageChunk(chunk)
            | SessionUpdate::AgentMessageChunk(chunk)
            | SessionUpdate::AgentThoughtChunk(chunk) => {
                let id = chunk
                    .message_id
                    .take()
                    .expect("replayed chunk has a message ID");
                let next = format!("m{}", message_ids.len());
                chunk.message_id = Some(message_ids.entry(id).or_insert(next).clone().into());
                continue;
            }
            _ => continue,
        };
        *id = ToolCallId::new(ids.get(id).expect("previously announced call").clone());
    }
    notifications
}

fn chunk(text: &str, message_id: &'static str) -> ContentChunk {
    ContentChunk::new(ContentBlock::from(text)).message_id(MessageId::from(message_id))
}

fn expected(updates: Vec<SessionUpdate>) -> Vec<SessionNotification> {
    updates
        .into_iter()
        .map(|update| {
            SessionNotification::new(SessionId::new("s1"), update).meta(serde_json::Map::from_iter(
                [("isReplay".into(), json!(true))],
            ))
        })
        .collect()
}

fn tool(id: &str) -> ToolCall {
    ToolCall::new(ToolCallId::new(id), "read")
        .name("read")
        .status(ToolCallStatus::Pending)
        .raw_input(json!({"path": "a.png"}))
}

// Covers: parallel calls must keep their ids and order, and tool-image
// supplements must update the same card without erasing output or failed status.
// Owner: ACP replay mapping (the stdio test covers persistence of ordinary results)
#[test]
fn replay_pairs_out_of_order_results_and_accumulates_images() {
    for (ok, status) in [
        (true, ToolCallStatus::Completed),
        (false, ToolCallStatus::Failed),
    ] {
        let image = model::ImageContent {
            data: "YWJj".into(),
            mime_type: "image/png".into(),
        };
        let content = vec![ToolCallContent::from("output")];
        let mut with_image = content.clone();
        with_image.push(ToolCallContent::from(ContentBlock::Image(
            ImageContent::new("YWJj", "image/png"),
        )));
        let messages = vec![
            Message::user_text("read images"),
            Message::Assistant(vec![
                call_block("a"),
                model::ContentBlock::Text("between".into()),
                call_block("b"),
            ]),
            Message::ToolResult(ToolResult {
                id: "b".into(),
                ok,
                content: "output".into(),
            }),
            Message::tool_image_supplement("read", "b", vec![image]).unwrap(),
            Message::ToolResult(ToolResult {
                id: "a".into(),
                ok: true,
                content: String::new(),
            }),
        ];
        assert_eq!(
            replay(&messages),
            expected(vec![
                SessionUpdate::UserMessageChunk(chunk("read images", "m0")),
                SessionUpdate::ToolCall(tool("0")),
                SessionUpdate::AgentMessageChunk(chunk("between", "m1")),
                SessionUpdate::ToolCall(tool("1")),
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    ToolCallId::new("1"),
                    ToolCallUpdateFields::new()
                        .status(status)
                        .content(content)
                        .raw_output(json!("output"))
                )),
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    ToolCallId::new("1"),
                    ToolCallUpdateFields::new()
                        .status(status)
                        .content(with_image)
                )),
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    ToolCallId::new("0"),
                    ToolCallUpdateFields::new()
                        .status(ToolCallStatus::Completed)
                        .content(vec![])
                        .raw_output(json!(""))
                )),
            ])
        );
    }
}

// Covers: interrupted history must not leave a spinning card, including calls
// in enriched/aborted assistants; partial results must never update unknown ids.
// Owner: ACP replay mapping
#[test]
fn replay_closes_incomplete_calls_and_opens_orphan_results() {
    for message in [
        Message::Assistant(vec![call_block("a")]),
        Message::EnrichedAssistant(Box::new(AssistantMessage::from_content(vec![call_block(
            "a",
        )]))),
        Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: vec![call_block("a")],
            ..Default::default()
        })),
    ] {
        assert_eq!(
            replay(&[message]),
            expected(vec![
                SessionUpdate::ToolCall(tool("0")),
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    ToolCallId::new("0"),
                    ToolCallUpdateFields::new().status(ToolCallStatus::Failed)
                )),
            ])
        );
    }
    for (ok, status) in [
        (true, ToolCallStatus::Completed),
        (false, ToolCallStatus::Failed),
    ] {
        assert_eq!(
            replay(&[Message::ToolResult(ToolResult {
                id: "orphan".into(),
                ok,
                content: "output".into()
            })]),
            expected(vec![SessionUpdate::ToolCall(
                ToolCall::new(ToolCallId::new("0"), "tool")
                    .status(status)
                    .content(vec![ToolCallContent::from("output")])
                    .raw_output(json!("output"))
            )])
        );
    }
}

// Covers: a reused provider ID must close the superseded unfinished occurrence,
// and later results/images and end-of-history closure must target the new cards.
// Owner: ACP replay mapping; the stdio test covers ordinary completed reuse.
#[test]
fn replay_closes_superseded_occurrences_without_retargeting_their_images() {
    let image = model::ImageContent {
        data: "YWJj".into(),
        mime_type: "image/png".into(),
    };
    let image_content =
        ToolCallContent::from(ContentBlock::Image(ImageContent::new("YWJj", "image/png")));
    let messages = vec![
        Message::Assistant(vec![call_block("reused")]),
        Message::tool_image_supplement("read", "reused", vec![image.clone()]).unwrap(),
        Message::Assistant(vec![call_block("reused")]),
        Message::ToolResult(ToolResult {
            id: "reused".into(),
            ok: false,
            content: "second output".into(),
        }),
        Message::tool_image_supplement("read", "reused", vec![image]).unwrap(),
        Message::Assistant(vec![call_block("reused")]),
    ];
    assert_eq!(
        replay(&messages),
        expected(vec![
            SessionUpdate::ToolCall(tool("0")),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("0"),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Pending)
                    .content(vec![image_content.clone()])
            )),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("0"),
                ToolCallUpdateFields::new().status(ToolCallStatus::Failed)
            )),
            SessionUpdate::ToolCall(tool("1")),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("1"),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Failed)
                    .content(vec![ToolCallContent::from("second output")])
                    .raw_output(json!("second output"))
            )),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("1"),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Failed)
                    .content(vec![ToolCallContent::from("second output"), image_content])
            )),
            SessionUpdate::ToolCall(tool("2")),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("2"),
                ToolCallUpdateFields::new().status(ToolCallStatus::Failed)
            )),
        ])
    );
}

// Covers: replayed chunks carry message IDs that split where live ones do
// (text after a tool call, a reasoning summary), give equal text saved twice
// two IDs, and come out the same when the same history replays again.
// Owner: ACP replay mapping; the stdio test covers reloading a saved session.
#[test]
fn replay_message_ids_split_at_boundaries_and_repeat_across_replays() {
    let assistant = AssistantMessage {
        reasoning_summary: Some("why".into()),
        ..AssistantMessage::from_content(vec![
            model::ContentBlock::Text("before".into()),
            call_block("a"),
            model::ContentBlock::Text("after".into()),
            model::ContentBlock::Text(" more".into()),
        ])
    };
    let messages = vec![
        Message::user_text("continue"),
        Message::assistant(assistant),
        Message::ToolResult(ToolResult {
            id: "a".into(),
            ok: true,
            content: String::new(),
        }),
        Message::user_text("continue"),
    ];
    let chunks = |notifications: Vec<SessionNotification>| {
        notifications
            .into_iter()
            .filter_map(|notification| match notification.update {
                SessionUpdate::UserMessageChunk(chunk)
                | SessionUpdate::AgentMessageChunk(chunk)
                | SessionUpdate::AgentThoughtChunk(chunk) => Some(chunk),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        chunks(replay(&messages)),
        vec![
            chunk("continue", "m0"),
            chunk("before", "m1"),
            chunk("after", "m2"),
            chunk(" more", "m2"),
            chunk("why", "m3"),
            chunk("continue", "m4"),
        ]
    );
    assert_eq!(chunks(raw_replay(&messages)), chunks(raw_replay(&messages)));
}
