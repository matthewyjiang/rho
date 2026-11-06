use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ImageContent, MessageId, SessionId, SessionNotification,
    SessionUpdate, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind,
};
use pretty_assertions::assert_eq;
use rho_sdk::model::{self, AbortedAssistant, AssistantMessage, Message, ToolResult};
use serde_json::json;

use super::history;
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::session_runtime::SessionTools,
    config::Config,
    diagnostics::RuntimeDiagnostics,
    session::tree::{DisplayRowOrigin, NodeId, ProjectedMessage},
    tools::sdk_registry::{AppToolSet, ToolSetOptions},
};

fn call_block(id: &str) -> model::ContentBlock {
    named_call_block(id, "read", json!({"path": "a.png"}))
}

fn named_call_block(id: &str, name: &str, arguments: serde_json::Value) -> model::ContentBlock {
    model::ContentBlock::ToolCall(model::ToolCall::new(id.into(), name.into(), arguments))
}

/// Replays `messages` as if each was saved as its own display row.
fn raw_replay(messages: &[Message]) -> Vec<SessionNotification> {
    history(
        &SessionId::new("s1"),
        &saved_rows(messages),
        std::path::Path::new(""),
        SessionTools::of_tool_set_for_tests(&AppToolSet::disabled()),
    )
}

fn saved_rows(messages: &[Message]) -> Vec<ProjectedMessage> {
    messages
        .iter()
        .enumerate()
        .map(|(row, message)| ProjectedMessage {
            origin: Some(DisplayRowOrigin {
                node: NodeId::from_string("node").unwrap(),
                row,
            }),
            message: message.clone(),
        })
        .collect()
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

// Covers: parallel calls must keep their ids and order, and a result's images
// land on its card after the output without changing its status.
// Owner: ACP replay mapping (the stdio test covers persistence of ordinary results)
#[test]
fn replay_pairs_out_of_order_results_with_their_images() {
    for (result, status) in [
        (
            ToolResult::succeeded("b", "output"),
            ToolCallStatus::Completed,
        ),
        (ToolResult::failed("b", "output"), ToolCallStatus::Failed),
    ] {
        let image = model::ImageContent {
            data: "YWJj".into(),
            mime_type: "image/png".into(),
        };
        let messages = vec![
            Message::user_text("read images"),
            Message::Assistant(vec![
                call_block("a"),
                model::ContentBlock::Text("between".into()),
                call_block("b"),
            ]),
            Message::ToolResult(result.with_images(vec![image])),
            Message::ToolResult(ToolResult::succeeded("a", String::new())),
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
                        .content(vec![
                            ToolCallContent::from("output"),
                            ToolCallContent::from(ContentBlock::Image(ImageContent::new(
                                "YWJj",
                                "image/png",
                            ))),
                        ])
                        .raw_output(json!("output"))
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
    for (result, status) in [
        (
            ToolResult::succeeded("orphan", "output"),
            ToolCallStatus::Completed,
        ),
        (
            ToolResult::failed("orphan", "output"),
            ToolCallStatus::Failed,
        ),
    ] {
        assert_eq!(
            replay(&[Message::ToolResult(result)]),
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
// and later results and end-of-history closure must target the new cards.
// Owner: ACP replay mapping; the stdio test covers ordinary completed reuse.
#[test]
fn replay_closes_superseded_occurrences() {
    let messages = vec![
        Message::Assistant(vec![call_block("reused")]),
        Message::Assistant(vec![call_block("reused")]),
        Message::ToolResult(
            ToolResult::failed("reused", "second output").with_images(vec![model::ImageContent {
                data: "YWJj".into(),
                mime_type: "image/png".into(),
            }]),
        ),
        Message::Assistant(vec![call_block("reused")]),
    ];
    assert_eq!(
        replay(&messages),
        expected(vec![
            SessionUpdate::ToolCall(tool("0")),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("0"),
                ToolCallUpdateFields::new().status(ToolCallStatus::Failed)
            )),
            SessionUpdate::ToolCall(tool("1")),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new("1"),
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Failed)
                    .content(vec![
                        ToolCallContent::from("second output"),
                        ToolCallContent::from(ContentBlock::Image(ImageContent::new(
                            "YWJj",
                            "image/png",
                        ))),
                    ])
                    .raw_output(json!("second output"))
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
// (text after a tool call, a reasoning summary), come in live order (the
// summary before the text it led to), give equal text saved twice two IDs,
// and come out the same when the same history replays again.
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
        Message::ToolResult(ToolResult::succeeded("a", String::new())),
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
            chunk("why", "m1"),
            chunk("before", "m2"),
            chunk("after", "m3"),
            chunk(" more", "m3"),
            chunk("continue", "m4"),
        ]
    );
    assert_eq!(chunks(raw_replay(&messages)), chunks(raw_replay(&messages)));
}

// Covers: a replayed call of a registered tool gets the kind its tool reports
// at start, as a live call does, including a tool that builds its metadata
// while preparing (`sessions`); an unknown tool stays `other`. The tools are
// ones every platform registers.
// Owner: ACP replay mapping; saved history keeps no tool metadata.
#[test]
fn replayed_calls_take_their_kind_from_the_session_tools() {
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(AgentCapabilities::new(
            [
                ToolCapability::ReadFile,
                ToolCapability::Grep,
                ToolCapability::Sessions,
            ]
            .into(),
        )),
    );
    let messages = saved_rows(&[Message::assistant(AssistantMessage::from_content(vec![
        named_call_block("a", "read_file", json!({"path": "src/lib.rs"})),
        named_call_block("b", "grep", json!({"pattern": "fn main"})),
        named_call_block("s", "sessions", json!({"action": "search", "query": "x"})),
        named_call_block("c", "not_a_tool", json!({})),
    ]))]);
    let kinds: Vec<ToolKind> = history(
        &SessionId::new("s1"),
        &messages,
        std::path::Path::new(""),
        SessionTools::of_tool_set_for_tests(&tool_set),
    )
    .into_iter()
    .filter_map(|notification| match notification.update {
        SessionUpdate::ToolCall(call) => Some(call.kind),
        _ => None,
    })
    .collect();
    assert_eq!(
        kinds,
        vec![
            ToolKind::Read,
            ToolKind::Search,
            ToolKind::Read,
            ToolKind::Other
        ]
    );
}
