use pretty_assertions::assert_eq;
use prost::Message;

use crate::model::{ContentBlock, Message as ChatMessage, ModelRequest, ToolCall, ToolResult};
use crate::protocol::cursor::proto::{
    agent_client_message, conversation_action, AgentClientMessage, AgentRunRequest,
};
use crate::reasoning::ReasoningLevel;

use super::build_cursor_turn;
use crate::protocol::cursor::connect::ConnectFrameParser;
use crate::protocol::cursor::effort::CursorEffort;
use crate::protocol::cursor::fast::CursorSpeed;

fn request<'a>(messages: &'a [ChatMessage], prompt_cache_key: Option<&'a str>) -> ModelRequest<'a> {
    ModelRequest {
        messages,
        tools: &[],
        cancellation: Default::default(),
        reasoning_level: Default::default(),
        prompt_cache_key,
    }
}

fn decode_run(bytes: &[u8]) -> AgentClientMessage {
    let frames = ConnectFrameParser::default().push(bytes).unwrap();
    AgentClientMessage::decode(frames[0].payload.as_slice()).unwrap()
}

fn run_action(bytes: &[u8]) -> conversation_action::Action {
    match decode_run(bytes).message {
        Some(agent_client_message::Message::RunRequest(run)) => run.action.unwrap().action.unwrap(),
        other => panic!("expected run request, got {other:?}"),
    }
}

fn run_model_id(bytes: &[u8]) -> String {
    match decode_run(bytes).message {
        Some(agent_client_message::Message::RunRequest(run)) => {
            run.requested_model.unwrap().model_id
        }
        other => panic!("expected run request, got {other:?}"),
    }
}

fn run_conversation_id(bytes: &[u8]) -> String {
    match decode_run(bytes).message {
        Some(agent_client_message::Message::RunRequest(run)) => run.conversation_id.unwrap(),
        other => panic!("expected run request, got {other:?}"),
    }
}

// Covers: a trailing user turn must be a UserMessageAction, and auto maps to wire id default
// Owner: cursor protocol
#[test]
fn trailing_user_message_uses_user_action_and_maps_auto_model() {
    let messages = [ChatMessage::user_text("hello")];
    let turn = build_cursor_turn(
        "auto",
        request(&messages, None),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();

    assert!(matches!(
        run_action(&turn.request_bytes),
        conversation_action::Action::UserMessageAction(action)
            if action.user_message.as_ref().is_some_and(|message| message.text == "hello")
    ));
    assert_eq!(run_model_id(&turn.request_bytes), "default");
}

// Covers: a follow-up after tool results must be a fresh user action, not Resume
// Owner: cursor protocol
#[test]
fn trailing_tool_result_uses_user_action_not_resume() {
    let messages = [
        ChatMessage::user_text("list files"),
        ChatMessage::assistant_text("calling read"),
        ChatMessage::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "src/lib.rs".into(),
        }),
    ];
    let turn = build_cursor_turn(
        "composer-1",
        request(&messages, None),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();

    assert!(matches!(
        run_action(&turn.request_bytes),
        conversation_action::Action::UserMessageAction(action)
            if action.user_message.as_ref().is_some_and(|message| {
                message.text.contains("call-1") && message.text.contains("src/lib.rs")
            })
    ));
    assert_eq!(run_model_id(&turn.request_bytes), "composer-1");
}

// Covers: /fast must land as a trailing -fast wire id, not a service-tier field
// Owner: cursor protocol
#[test]
fn fast_speed_appends_trailing_fast_suffix_on_run() {
    let messages = [ChatMessage::user_text("hello")];
    let turn = build_cursor_turn(
        "grok-4.6-high",
        request(&messages, None),
        CursorSpeed::Fast,
        CursorEffort::Unspecified,
    )
    .unwrap();

    assert_eq!(run_model_id(&turn.request_bytes), "grok-4.6-high-fast");
}

// Covers: selected xhigh must compose with Fast as grok-4.6-xhigh-fast
// Owner: cursor protocol
#[test]
fn xhigh_fast_composes_effort_then_speed_on_run() {
    let messages = [ChatMessage::user_text("hello")];
    let turn = build_cursor_turn(
        "grok-4.6",
        request(&messages, None),
        CursorSpeed::Fast,
        CursorEffort::Level(ReasoningLevel::Xhigh),
    )
    .unwrap();

    assert_eq!(run_model_id(&turn.request_bytes), "grok-4.6-xhigh-fast");
}

// Covers: conversation id must be unique per Run so Cursor cannot resume a torn-down MCP call
// Owner: cursor protocol
#[test]
fn conversation_id_is_unique_per_run_not_session_key() {
    let messages = [ChatMessage::user_text("fix the tests")];

    let first = build_cursor_turn(
        "auto",
        request(&messages, Some("session-1")),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();
    let second = build_cursor_turn(
        "auto",
        request(&messages, Some("session-1")),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();

    assert_ne!(
        run_conversation_id(&first.request_bytes),
        run_conversation_id(&second.request_bytes)
    );
}

// Covers: rebuilt history must keep tool name/args or the model repeats the same edit
// Owner: cursor protocol
#[test]
fn assistant_tool_calls_are_kept_in_rebuilt_history() {
    let messages = [
        ChatMessage::user_text("update foo.rs"),
        ChatMessage::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-9".into(),
            name: "str_replace".into(),
            arguments: serde_json::json!({
                "path": "foo.rs",
                "old_string": "a",
                "new_string": "b",
            }),
        })]),
        ChatMessage::ToolResult(ToolResult {
            id: "call-9".into(),
            ok: true,
            content: "updated".into(),
        }),
    ];
    let turn = build_cursor_turn(
        "auto",
        request(&messages, None),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();

    let prompt_blobs = prompt_json_texts(&turn);
    assert!(
        prompt_blobs.iter().any(|blob| {
            blob.contains("[Called str_replace id=call-9]") && blob.contains("foo.rs")
        }),
        "rebuilt prompt blobs lost the tool call: {prompt_blobs:?}"
    );
}

fn prompt_json_texts(turn: &super::CursorTurn) -> Vec<String> {
    let run = match decode_run(&turn.request_bytes).message {
        Some(agent_client_message::Message::RunRequest(run)) => run,
        other => panic!("expected run request, got {other:?}"),
    };
    let AgentRunRequest {
        conversation_state: Some(state),
        ..
    } = run
    else {
        panic!("expected conversation state");
    };
    state
        .root_prompt_messages_json
        .iter()
        .filter_map(|id| turn.blob_store.get(id))
        .filter_map(|bytes| std::str::from_utf8(bytes).ok().map(str::to_owned))
        .collect()
}
