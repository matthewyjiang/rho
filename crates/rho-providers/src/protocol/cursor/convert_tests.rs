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
use crate::reasoning::ReasoningLevel;

use super::value::protobuf_value_from_json;
use super::{
    build_cursor_turn, decode_mcp_args, fallback_models, models_from_details, native_exec_reject,
    ConnectFrameParser, CursorEffort, CursorSpeed,
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
    let turn = build_cursor_turn(
        &identity(),
        "auto",
        request(&messages, &[]),
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
        &identity(),
        "composer-1",
        request(&messages, &[]),
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

// Covers: protobuf NumberValue is always f64; bash timeout_seconds must decode as u64
// Owner: cursor protocol
#[test]
fn mcp_args_decode_whole_numbers_as_json_integers() {
    let mut args = McpArgs {
        name: "bash".into(),
        tool_name: String::new(),
        tool_call_id: "call-timeout".into(),
        provider_identifier: "rho".into(),
        args: Default::default(),
    };
    args.args.insert(
        "timeout_seconds".into(),
        protobuf_value_from_json(&serde_json::Value::Number(
            serde_json::Number::from_f64(30.0).expect("finite"),
        ))
        .encode_to_vec(),
    );
    args.args.insert(
        "command".into(),
        protobuf_value_from_json(&json!("true")).encode_to_vec(),
    );

    let call = decode_mcp_args(&args).unwrap();
    assert_eq!(call.arguments["timeout_seconds"].as_u64(), Some(30));
    assert_eq!(call.arguments["command"], json!("true"));
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
    assert!(models[1].reasoning_levels.is_empty());
}

// Covers: Fast variants must collapse to the base catalog id so /fast is the switch
// Owner: cursor protocol
#[test]
fn discovered_fast_variants_collapse_to_the_base_model() {
    let models = models_from_details(&[
        ModelDetails {
            model_id: "grok-4.6-high-fast".into(),
            display_model_id: String::new(),
            display_name: "Grok 4.6 Fast".into(),
            display_name_short: String::new(),
            thinking_details: None,
        },
        ModelDetails {
            model_id: "grok-4.6-high".into(),
            display_model_id: String::new(),
            display_name: "Grok 4.6".into(),
            display_name_short: String::new(),
            thinking_details: Some(ThinkingDetails {}),
        },
        ModelDetails {
            model_id: "grok-code-fast-1".into(),
            display_model_id: String::new(),
            display_name: "Grok Code Fast 1".into(),
            display_name_short: String::new(),
            thinking_details: None,
        },
    ]);

    let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, ["auto", "grok-4.6", "grok-code-fast-1"]);
    let grok = models.iter().find(|model| model.id == "grok-4.6").unwrap();
    assert_eq!(grok.name, "Grok 4.6");
    assert_eq!(grok.reasoning_levels, vec![ReasoningLevel::High]);
}

// Covers: detected effort suffixes are the only picker levels, including xhigh when present
// Owner: cursor protocol
#[test]
fn discovered_effort_suffixes_become_picker_levels() {
    let cases: &[(&[&str], &str, &[ReasoningLevel])] = &[
        (
            &[
                "grok-4.6-low",
                "grok-4.6-high",
                "grok-4.6-xhigh",
                "grok-4.6-xhigh-fast",
            ],
            "grok-4.6",
            &[
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Xhigh,
            ],
        ),
        (
            &["grok-4.6-high", "grok-4.6-medium"],
            "grok-4.6",
            &[ReasoningLevel::Medium, ReasoningLevel::High],
        ),
        (&["composer-1"], "composer-1", &[]),
    ];

    for (ids, catalog, levels) in cases {
        let models = models_from_details(
            &ids.iter()
                .map(|id| ModelDetails {
                    model_id: (*id).into(),
                    display_model_id: String::new(),
                    display_name: catalog.to_string(),
                    display_name_short: String::new(),
                    thinking_details: None,
                })
                .collect::<Vec<_>>(),
        );
        let model = models.iter().find(|model| model.id == *catalog).unwrap();
        assert_eq!(model.reasoning_levels.as_slice(), *levels, "ids={ids:?}");
    }
}

// Covers: fallback raw ids go through the same suffix detector, so grok 4.6 xhigh is pickable offline
// Owner: cursor protocol
#[test]
fn fallback_detects_grok_46_xhigh_from_suffixed_ids() {
    let grok = fallback_models()
        .into_iter()
        .find(|model| model.id == "grok-4.6")
        .unwrap();
    assert_eq!(
        grok.reasoning_levels,
        vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ]
    );
}

// Covers: /fast must land as a trailing -fast wire id, not a service-tier field
// Owner: cursor protocol
#[test]
fn fast_speed_appends_trailing_fast_suffix_on_run() {
    let messages = [ChatMessage::user_text("hello")];
    let turn = build_cursor_turn(
        &identity(),
        "grok-4.6-high",
        request(&messages, &[]),
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
        &identity(),
        "grok-4.6",
        request(&messages, &[]),
        CursorSpeed::Fast,
        CursorEffort::Level(ReasoningLevel::Xhigh),
    )
    .unwrap();

    assert_eq!(run_model_id(&turn.request_bytes), "grok-4.6-xhigh-fast");
}
