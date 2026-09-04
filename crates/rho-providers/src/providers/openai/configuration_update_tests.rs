use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::*;
use crate::model::{
    AssistantMessage, ContentBlock, Message, ModelIdentity, ModelRequest, ProviderContextBlock,
    ToolCall, ToolResult,
};
use crate::reasoning::ReasoningLevel;

use super::super::auth::Auth;
use super::super::codex_request::{
    build_responses_compact_body, build_responses_create_body, codex_test_auth,
    ResponsesCreateBody, ResponsesProfile,
};
use super::super::reasoning::OpenAiReasoningProfile;

fn identity_for(model: &str) -> ModelIdentity {
    ResponsesProfile::from_auth(&codex_test_auth(), model)
        .identity()
        .clone()
}

fn effort_block(identity: &ModelIdentity, effort: &str) -> ProviderContextBlock {
    ProviderContextBlock {
        identity: identity.clone(),
        kind: OPENAI_REASONING_EFFORT_KIND.into(),
        position: None,
        data: json!(effort),
    }
}

fn assistant_text(identity: &ModelIdentity, text: &str, effort: Option<&str>) -> Message {
    Message::assistant(AssistantMessage {
        content: vec![ContentBlock::Text(text.into())],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: effort
            .map(|effort| vec![effort_block(identity, effort)])
            .unwrap_or_default(),
    })
}

fn assistant_tool_call(identity: &ModelIdentity, effort: &str) -> Message {
    Message::assistant(AssistantMessage {
        content: vec![ContentBlock::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: json!({}),
        })],
        provenance: Some(identity.clone()),
        reasoning_summary: None,
        provider_context: vec![effort_block(identity, effort)],
    })
}

fn request<'a>(messages: &'a [Message], level: ReasoningLevel) -> ModelRequest<'a> {
    ModelRequest {
        messages,
        tools: &[],
        cancellation: Default::default(),
        reasoning_level: level,
        prompt_cache_key: None,
    }
}

fn create_body(model: &str, messages: &[Message], level: ReasoningLevel) -> ResponsesCreateBody {
    let profile = ResponsesProfile::from_auth(&codex_test_auth(), model);
    build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request(messages, level),
        None,
        /*hosted_web_search*/ true,
    )
    .expect("create body")
}

fn compact_body(model: &str, messages: &[Message], level: ReasoningLevel) -> Value {
    let profile = ResponsesProfile::from_auth(&codex_test_auth(), model);
    build_responses_compact_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request(messages, level),
    )
    .expect("compact body")
}

fn user_item(text: &str) -> Value {
    json!({"role":"user","content":[{"type":"input_text","text":text}]})
}

fn assistant_item(text: &str) -> Value {
    json!({"role":"assistant","content":text})
}

fn update_item(effort: &str) -> Value {
    json!({"type":"configuration_update","reasoning":{"effort":effort}})
}

fn assert_no_adjacent_updates(input: &[Value]) {
    assert!(
        !has_adjacent_configuration_updates(input),
        "configuration_update items must be followed by a real input item: {input:?}"
    );
}

// Covers: astra create bodies freeze prefix effort and insert configuration_update
// Owner: openai configuration_update lowering
#[test]
fn astra_create_bodies_preserve_prefix_effort() {
    let astra = identity_for("gpt-6-astra");
    struct Case {
        name: &'static str,
        messages: Vec<Message>,
        level: ReasoningLevel,
        expected_effort: &'static str,
        expected_in_force: &'static str,
        expected_input: Vec<Value>,
    }

    let cases = [
        Case {
            name: "update before trailing user when level changes",
            messages: vec![
                Message::user_text("one"),
                assistant_text(&astra, "two", Some("low")),
                Message::user_text("three"),
            ],
            level: ReasoningLevel::High,
            expected_effort: "low",
            expected_in_force: "high",
            expected_input: vec![
                user_item("one"),
                assistant_item("two"),
                update_item("high"),
                user_item("three"),
            ],
        },
        Case {
            name: "no update when unchanged",
            messages: vec![
                Message::user_text("one"),
                assistant_text(&astra, "two", Some("low")),
                Message::user_text("three"),
            ],
            level: ReasoningLevel::Low,
            expected_effort: "low",
            expected_in_force: "low",
            expected_input: vec![user_item("one"), assistant_item("two"), user_item("three")],
        },
        Case {
            name: "update before trailing tool-result batch",
            messages: vec![
                Message::user_text("read it"),
                assistant_tool_call(&astra, "low"),
                Message::ToolResult(ToolResult {
                    id: "call_1".into(),
                    ok: true,
                    content: "contents".into(),
                }),
            ],
            level: ReasoningLevel::High,
            expected_effort: "low",
            expected_in_force: "high",
            expected_input: vec![
                user_item("read it"),
                json!({
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "read",
                    "arguments": "{}",
                }),
                update_item("high"),
                json!({
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "contents",
                }),
            ],
        },
        Case {
            name: "historical updates replay at original positions",
            messages: vec![
                Message::user_text("one"),
                assistant_text(&astra, "two", Some("low")),
                Message::user_text("three"),
                assistant_text(&astra, "four", Some("high")),
                Message::user_text("five"),
            ],
            level: ReasoningLevel::High,
            expected_effort: "low",
            expected_in_force: "high",
            expected_input: vec![
                user_item("one"),
                assistant_item("two"),
                update_item("high"),
                user_item("three"),
                assistant_item("four"),
                user_item("five"),
            ],
        },
        Case {
            name: "no records uses current level and emits no updates",
            messages: vec![
                Message::user_text("one"),
                assistant_text(&astra, "two", None),
                Message::user_text("three"),
            ],
            level: ReasoningLevel::High,
            expected_effort: "high",
            expected_in_force: "high",
            expected_input: vec![user_item("one"), assistant_item("two"), user_item("three")],
        },
        Case {
            name:
                "history ending on assistant emits no trailing update and keeps baseline in force",
            messages: vec![
                Message::user_text("one"),
                assistant_text(&astra, "two", Some("low")),
            ],
            level: ReasoningLevel::High,
            expected_effort: "low",
            expected_in_force: "low",
            expected_input: vec![user_item("one"), assistant_item("two")],
        },
    ];

    for case in cases {
        let created = create_body("gpt-6-astra", &case.messages, case.level);
        let input = created.body["input"].as_array().expect(case.name);
        assert_no_adjacent_updates(input);
        assert_eq!(
            created.body["reasoning"]["effort"], case.expected_effort,
            "{}",
            case.name
        );
        assert_eq!(
            created.in_force_effort.as_deref(),
            Some(case.expected_in_force),
            "{}",
            case.name
        );
        assert_eq!(*input, case.expected_input, "{}", case.name);
    }
}

// Covers: non-astra models keep request-level effort on the current turn
// Owner: openai configuration_update lowering
#[test]
fn non_astra_create_body_follows_current_level_without_updates() {
    let gpt55 = identity_for("gpt-5.5");
    let messages = [
        Message::user_text("one"),
        assistant_text(&gpt55, "two", Some("low")),
        Message::user_text("three"),
    ];
    let created = create_body("gpt-5.5", &messages, ReasoningLevel::High);
    let input = created.body["input"].as_array().expect("input");
    assert_eq!(created.body["reasoning"]["effort"], "high");
    assert_eq!(created.in_force_effort.as_deref(), Some("high"));
    assert!(input.iter().all(|item| !is_configuration_update_item(item)));
    assert_eq!(
        *input,
        vec![user_item("one"), assistant_item("two"), user_item("three")]
    );
}

// Covers: compact rejects configuration_update, so it never emits those items
// Owner: openai configuration_update lowering
#[test]
fn compact_body_omits_configuration_update_even_when_create_would_emit() {
    let astra = identity_for("gpt-6-astra");
    let messages = [
        Message::user_text("one"),
        assistant_text(&astra, "two", Some("low")),
        Message::user_text("three"),
    ];
    let compact = compact_body("gpt-6-astra", &messages, ReasoningLevel::High);
    let input = compact["input"].as_array().expect("input");
    assert_eq!(compact["reasoning"]["effort"], "high");
    assert!(input.iter().all(|item| !is_configuration_update_item(item)));
    assert_eq!(
        *input,
        vec![user_item("one"), assistant_item("two"), user_item("three")]
    );
}

// Covers: API-key Responses identity also records and freezes prefix effort
// Owner: openai configuration_update lowering
#[test]
fn api_key_astra_create_body_emits_configuration_update() {
    let profile = ResponsesProfile::from_auth(&Auth::ApiKey("key".into()), "gpt-6-astra");
    let identity = profile.identity().clone();
    let messages = [
        Message::user_text("one"),
        assistant_text(&identity, "two", Some("medium")),
        Message::user_text("three"),
    ];
    let body = build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request(&messages, ReasoningLevel::Max),
        None,
        /*hosted_web_search*/ true,
    )
    .unwrap();
    assert_eq!(body.body["reasoning"]["effort"], "medium");
    assert_eq!(body.in_force_effort.as_deref(), Some("max"));
    assert_eq!(
        body.body["input"],
        json!([
            user_item("one"),
            assistant_item("two"),
            update_item("max"),
            user_item("three"),
        ])
    );
}
