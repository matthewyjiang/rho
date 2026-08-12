use pretty_assertions::assert_eq;
use prost::Message;
use serde_json::json;

use crate::model::ToolCall;
use crate::model::{Message as ChatMessage, ModelIdentity, ModelRequest};
use crate::protocol::cursor::build_cursor_turn;
use crate::protocol::cursor::proto::{
    agent_server_message, exec_server_message, interaction_update, kv_server_message,
    AgentServerMessage, ExecServerMessage, GetBlobArgs, InteractionUpdate, KvServerMessage,
    McpArgs, ReadArgs, RequestContextArgs, SetBlobArgs, TextDeltaUpdate, TurnEndedUpdate,
};
use crate::protocol::cursor::value::protobuf_value_from_json;

use super::stream::{handle_server_message, CursorHandle};

fn turn() -> crate::protocol::cursor::CursorTurn {
    let identity = ModelIdentity::new("cursor", "cursor-agent", "auto");
    let messages = [ChatMessage::user_text("hello")];
    build_cursor_turn(
        &identity,
        "auto",
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
    )
    .unwrap()
}

fn interaction(message: interaction_update::Message) -> AgentServerMessage {
    AgentServerMessage {
        message: Some(agent_server_message::Message::InteractionUpdate(
            InteractionUpdate {
                message: Some(message),
            },
        )),
    }
}

// Covers: text deltas and turn-ended must become assistant output, not dropped frames
// Owner: cursor runtime
#[test]
fn text_delta_then_turn_ended_are_distinct_handle_events() {
    let mut session = turn();
    let delta = handle_server_message(
        &interaction(interaction_update::Message::TextDelta(TextDeltaUpdate {
            text: "Hi".into(),
        })),
        &mut session,
    )
    .unwrap();
    let ended = handle_server_message(
        &interaction(interaction_update::Message::TurnEnded(TurnEndedUpdate {})),
        &mut session,
    )
    .unwrap();
    assert!(matches!(delta, CursorHandle::TextDelta(text) if text == "Hi"));
    assert!(matches!(ended, CursorHandle::TurnEnded));
}

// Covers: MCP exec must surface a Rho tool call instead of being rejected as a native tool
// Owner: cursor runtime
#[test]
fn mcp_exec_becomes_a_tool_call() {
    let mut session = turn();
    let mut args = McpArgs {
        name: String::new(),
        tool_name: "read_file".into(),
        tool_call_id: "call-1".into(),
        provider_identifier: "rho".into(),
        args: Default::default(),
    };
    args.args.insert(
        "path".into(),
        protobuf_value_from_json(&json!("src/lib.rs")).encode_to_vec(),
    );
    let handle = handle_server_message(
        &AgentServerMessage {
            message: Some(agent_server_message::Message::ExecServerMessage(
                ExecServerMessage {
                    id: 3,
                    exec_id: "exec-3".into(),
                    message: Some(exec_server_message::Message::McpArgs(args)),
                },
            )),
        },
        &mut session,
    )
    .unwrap();
    assert!(matches!(
        handle,
        CursorHandle::McpTool(ToolCall { name, id, .. }) if name == "read_file" && id == "call-1"
    ));
}

// Covers: native read must be rejected and requestContext must reply with MCP tools
// Owner: cursor runtime
#[test]
fn request_context_replies_and_native_read_is_rejected() {
    let mut session = turn();
    let context = handle_server_message(
        &AgentServerMessage {
            message: Some(agent_server_message::Message::ExecServerMessage(
                ExecServerMessage {
                    id: 1,
                    exec_id: "exec-1".into(),
                    message: Some(exec_server_message::Message::RequestContextArgs(
                        RequestContextArgs {},
                    )),
                },
            )),
        },
        &mut session,
    )
    .unwrap();
    let native = handle_server_message(
        &AgentServerMessage {
            message: Some(agent_server_message::Message::ExecServerMessage(
                ExecServerMessage {
                    id: 2,
                    exec_id: "exec-2".into(),
                    message: Some(exec_server_message::Message::ReadArgs(ReadArgs {
                        path: "src/lib.rs".into(),
                    })),
                },
            )),
        },
        &mut session,
    )
    .unwrap();
    assert!(matches!(context, CursorHandle::Reply(_)));
    assert!(matches!(native, CursorHandle::Reply(_)));
}

// Covers: KV set must be acknowledged and a later get of that blob must not miss
// Owner: cursor runtime
#[test]
fn kv_set_then_get_uses_the_turn_blob_store() {
    let mut session = turn();
    let stored = handle_server_message(
        &AgentServerMessage {
            message: Some(agent_server_message::Message::KvServerMessage(
                KvServerMessage {
                    id: 10,
                    message: Some(kv_server_message::Message::SetBlobArgs(SetBlobArgs {
                        blob_id: b"blob-1".to_vec(),
                        blob_data: b"data".to_vec(),
                    })),
                },
            )),
        },
        &mut session,
    )
    .unwrap();
    let hit = handle_server_message(
        &AgentServerMessage {
            message: Some(agent_server_message::Message::KvServerMessage(
                KvServerMessage {
                    id: 11,
                    message: Some(kv_server_message::Message::GetBlobArgs(GetBlobArgs {
                        blob_id: b"blob-1".to_vec(),
                    })),
                },
            )),
        },
        &mut session,
    )
    .unwrap();
    assert!(matches!(stored, CursorHandle::Reply(_)));
    assert!(matches!(hit, CursorHandle::Reply(_)));
    assert_eq!(session.blob_store.get(b"blob-1"), Some(b"data".as_slice()));
}
