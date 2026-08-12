use pretty_assertions::assert_eq;
use prost::Message;

use crate::model::{Message as ChatMessage, ModelRequest, ToolResult};
use crate::protocol::cursor::proto::{
    agent_client_message, conversation_action, AgentClientMessage,
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

// Covers: a follow-up after tool results must resume instead of sending a duplicate user message
// Owner: cursor protocol
#[test]
fn trailing_tool_result_uses_resume_action() {
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
        conversation_action::Action::ResumeAction(_)
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

// Covers: Cursor conversation id must follow the session key, not the first user text
// Owner: cursor protocol
#[test]
fn conversation_id_follows_prompt_cache_key_not_opener_text() {
    let first = [ChatMessage::user_text("fix the tests")];
    let second = [ChatMessage::user_text("fix the tests")];
    let other = [ChatMessage::user_text("something else")];

    let same_session_a = build_cursor_turn(
        "auto",
        request(&first, Some("session-1")),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();
    let same_session_b = build_cursor_turn(
        "auto",
        request(&second, Some("session-1")),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();
    let other_session = build_cursor_turn(
        "auto",
        request(&other, Some("session-2")),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();
    let same_opener_no_key = build_cursor_turn(
        "auto",
        request(&first, None),
        CursorSpeed::Standard,
        CursorEffort::Unspecified,
    )
    .unwrap();

    assert_eq!(
        run_conversation_id(&same_session_a.request_bytes),
        run_conversation_id(&same_session_b.request_bytes)
    );
    assert_ne!(
        run_conversation_id(&same_session_a.request_bytes),
        run_conversation_id(&other_session.request_bytes)
    );
    assert_ne!(
        run_conversation_id(&same_session_a.request_bytes),
        run_conversation_id(&same_opener_no_key.request_bytes)
    );
}
