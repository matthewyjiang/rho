use pretty_assertions::assert_eq;
use prost::Message;
use serde_json::json;

use super::proto::{
    agent_client_message, conversation_action, exec_server_message, AgentClientMessage,
    ExecServerMessage, McpArgs, ModelDetails, ReadArgs, RequestContextArgs, ThinkingDetails,
};
use crate::model::{
    Message as ChatMessage, ModelIdentity, ModelRequest, ToolCall, ToolResult, ToolSpec,
};

use super::value::protobuf_value_from_json;
use super::{
    build_cursor_turn, decode_mcp_args, models_from_details, native_exec_reject, ConnectFrameParser,
};

fn identity() -> ModelIdentity {
    ModelIdentity::new("cursor", "cursor-agent", "auto")
}

fn request<'a>(messages: &'a [ChatMessage], tools: &'a [ToolSpec]) -> ModelRequest<'a> {
    ModelRequest {
        messages,
        tools,
        cancellation: Default::default(),
        reasoning_level: Default::default(),
        prompt_cache_key: None,
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

// Covers: a trailing user turn must be a UserMessageAction, and auto maps to wire id default
// Owner: cursor protocol
#[test]
fn trailing_user_message_uses_user_action_and_maps_auto_model() {
    let messages = [ChatMessage::user_text("hello")];
    let turn = build_cursor_turn(&identity(), "auto", request(&messages, &[])).unwrap();

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
    let turn = build_cursor_turn(&identity(), "composer-1", request(&messages, &[])).unwrap();

    assert!(matches!(
        run_action(&turn.request_bytes),
        conversation_action::Action::ResumeAction(_)
    ));
    assert_eq!(run_model_id(&turn.request_bytes), "composer-1");
}

// Covers: MCP exec args are protobuf Values and must become a JSON object ToolCall
// Owner: cursor protocol
#[test]
fn mcp_args_decode_protobuf_values_into_tool_call_object() {
    let mut args = McpArgs {
        name: "read_file".into(),
        tool_name: String::new(),
        tool_call_id: "call-9".into(),
        provider_identifier: "rho".into(),
        args: Default::default(),
    };
    args.args.insert(
        "path".into(),
        protobuf_value_from_json(&json!("/tmp/a.rs")).encode_to_vec(),
    );

    assert_eq!(
        decode_mcp_args(&args).unwrap(),
        ToolCall {
            id: "call-9".into(),
            name: "read_file".into(),
            arguments: json!({ "path": "/tmp/a.rs" }),
        }
    );
}

// Covers: native Cursor tools must be rejected so the model falls back to Rho MCP tools
// Owner: cursor protocol
#[test]
fn native_exec_is_rejected_and_request_context_is_left_to_the_runtime() {
    let read = ExecServerMessage {
        id: 1,
        exec_id: "exec-1".into(),
        message: Some(exec_server_message::Message::ReadArgs(ReadArgs {
            path: "src/lib.rs".into(),
        })),
    };
    let context = ExecServerMessage {
        id: 2,
        exec_id: "exec-2".into(),
        message: Some(exec_server_message::Message::RequestContextArgs(
            RequestContextArgs {},
        )),
    };

    assert!(native_exec_reject(&read).is_some());
    assert!(native_exec_reject(&context).is_none());
}

// Covers: GetUsableModels rows missing auto still expose Cursor's Auto routing id
// Owner: cursor protocol
#[test]
fn discovered_models_always_include_auto() {
    let models = models_from_details(&[ModelDetails {
        model_id: "composer-1".into(),
        display_model_id: String::new(),
        display_name: "Composer 1".into(),
        display_name_short: String::new(),
        thinking_details: Some(ThinkingDetails {}),
    }]);

    assert_eq!(models[0].id, "auto");
    assert_eq!(models[1].id, "composer-1");
    assert!(models[1].reasoning);
}
