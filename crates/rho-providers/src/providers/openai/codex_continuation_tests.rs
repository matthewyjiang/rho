use super::*;
use crate::model::{ContentBlock, ModelResponse};
use serde_json::json;

fn body(input: Vec<Value>) -> Value {
    json!({
        "model": "gpt-5-codex",
        "instructions": "system",
        "input": input,
        "store": false,
        "stream": true,
        "tools": [{"type":"function","name":"read","parameters":{"type":"object"}}],
        "tool_choice": "auto",
        "reasoning": {"effort":"low","summary":"auto"},
    })
}

fn candidate(input: Vec<Value>) -> CodexContinuationCandidate {
    CodexContinuationCandidate::from_responses_body(&body(input)).unwrap()
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse::Assistant(vec![ContentBlock::Text(text.into())])
}

#[test]
fn continuation_uses_only_new_user_input_after_server_assistant_output() {
    let first = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"one"}]}),
    ]);
    let next = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"one"}]}),
        json!({"role":"assistant","content":"two"}),
        json!({"role":"user","content":[{"type":"input_text","text":"three"}]}),
    ]);
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &text_response("two"),
            Some("resp_1".into()),
            vec![json!({"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"two"}]})],
        ),
    );

    let plan = state
        .continuation_delta(&next)
        .expect("snapshot extends this turn");

    assert_eq!(plan["previous_response_id"], "resp_1");
    assert_eq!(
        plan["input"],
        json!([{"role":"user","content":[{"type":"input_text","text":"three"}]}])
    );
}

#[test]
fn continuation_retains_tool_result_after_server_function_call() {
    let first = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"read it"}]}),
    ]);
    let next = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"read it"}]}),
        json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"call_1","output":"contents"}),
    ]);
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(crate::model::ToolCall {
        id: "call_1".into(),
        name: "read".into(),
        arguments: json!({}),
    })]);
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &response,
            Some("resp_1".into()),
            vec![json!({"id":"fc_1","type":"function_call","call_id":"call_1","name":"read","arguments":"{}"})],
        ),
    );

    let plan = state
        .continuation_delta(&next)
        .expect("snapshot extends this turn");

    assert_eq!(plan["previous_response_id"], "resp_1");
    assert_eq!(
        plan["input"],
        json!([{"type":"function_call_output","call_id":"call_1","output":"contents"}])
    );
}

#[test]
fn continuation_accepts_semantically_equivalent_function_call_arguments() {
    let first = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"read it"}]}),
    ]);
    let next = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"read it"}]}),
        json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"line_end\":10,\"path\":\"README.md\"}"}),
        json!({"type":"function_call_output","call_id":"call_1","output":"contents"}),
    ]);
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(crate::model::ToolCall {
        id: "call_1".into(),
        name: "read".into(),
        arguments: json!({"line_end": 10, "path": "README.md"}),
    })]);
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &response,
            Some("resp_1".into()),
            vec![json!({
                "id":"fc_1",
                "type":"function_call",
                "call_id":"call_1",
                "name":"read",
                "arguments":"{ \"path\" : \"README.md\", \"line_end\" : 10 }",
            })],
        ),
    );

    let plan = state
        .continuation_delta(&next)
        .expect("snapshot extends this turn");

    assert_eq!(plan["previous_response_id"], "resp_1");
    assert_eq!(
        plan["input"],
        json!([{"type":"function_call_output","call_id":"call_1","output":"contents"}])
    );
}

#[test]
fn continuation_falls_back_to_full_request_when_server_output_is_unavailable() {
    let first = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"one"}]}),
    ]);
    let next = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"one"}]}),
        json!({"role":"assistant","content":"two"}),
        json!({"role":"user","content":[{"type":"input_text","text":"three"}]}),
    ]);
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &text_response("two"),
            Some("resp_1".into()),
            Vec::new(),
        ),
    );

    assert_eq!(state.continuation_delta(&next), None);
}

#[test]
fn continuation_falls_back_to_full_request_when_request_properties_change() {
    let first = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"one"}]}),
    ]);
    let next_body = json!({
        "model": "gpt-5-codex",
        "instructions": "changed system",
        "input": [
            {"role":"user","content":[{"type":"input_text","text":"one"}]},
            {"role":"assistant","content":"two"},
            {"role":"user","content":[{"type":"input_text","text":"three"}]},
        ],
        "store": false,
        "stream": true,
        "tools": [{"type":"function","name":"read","parameters":{"type":"object"}}],
        "tool_choice": "auto",
        "reasoning": {"effort":"low","summary":"auto"},
    });
    let next = CodexContinuationCandidate::from_responses_body(&next_body).unwrap();
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &text_response("two"),
            Some("resp_1".into()),
            vec![json!({"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"two"}]})],
        ),
    );

    assert_eq!(state.continuation_delta(&next), None);
}

#[test]
fn continuation_falls_back_to_full_request_for_unrepresentable_server_output() {
    let first = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"one"}]}),
    ]);
    let next = candidate(vec![
        json!({"role":"user","content":[{"type":"input_text","text":"one"}]}),
        json!({"role":"assistant","content":"two"}),
        json!({"role":"user","content":[{"type":"input_text","text":"three"}]}),
    ]);
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &text_response("two"),
            Some("resp_1".into()),
            vec![json!({"type":"web_search_call","id":"search_1"})],
        ),
    );

    assert_eq!(state.continuation_delta(&next), None);
}

// Covers: astra effort changes must stay on configuration_update so WS deltas keep working
// Owner: openai Codex continuation
#[test]
fn astra_reasoning_change_keeps_request_properties_and_deltas_update_item() {
    use crate::model::{AssistantMessage, Message, ModelRequest, ProviderContextBlock};
    use crate::reasoning::ReasoningLevel;
    use pretty_assertions::assert_eq;

    use super::super::codex_request::{
        build_responses_create_body, codex_test_auth, ResponsesProfile,
    };
    use super::super::configuration_update::OPENAI_REASONING_EFFORT_KIND;
    use super::super::reasoning::OpenAiReasoningProfile;

    let profile = ResponsesProfile::from_auth(&codex_test_auth(), "gpt-6-astra");
    let identity = profile.identity().clone();
    let first_messages = [Message::user_text("one")];
    let first = build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &first_messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::Low,
            prompt_cache_key: None,
        },
        None,
        /*hosted_web_search*/ true,
    )
    .unwrap()
    .body;
    let next_messages = [
        Message::user_text("one"),
        Message::assistant(AssistantMessage {
            content: vec![ContentBlock::Text("two".into())],
            provenance: Some(identity.clone()),
            reasoning_summary: None,
            provider_context: vec![ProviderContextBlock {
                identity,
                kind: OPENAI_REASONING_EFFORT_KIND.into(),
                position: None,
                data: json!("low"),
            }],
        }),
        Message::user_text("three"),
    ];
    let next = build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        ModelRequest {
            messages: &next_messages,
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: ReasoningLevel::High,
            prompt_cache_key: None,
        },
        None,
        /*hosted_web_search*/ true,
    )
    .unwrap()
    .body;

    let mut first_properties = first.clone();
    first_properties.as_object_mut().unwrap().remove("input");
    let mut next_properties = next.clone();
    next_properties.as_object_mut().unwrap().remove("input");
    assert_eq!(first_properties, next_properties);
    assert_eq!(first["reasoning"]["effort"], "low");
    assert_eq!(next["reasoning"]["effort"], "low");

    let first_candidate = CodexContinuationCandidate::from_responses_body(&first).unwrap();
    let next_candidate = CodexContinuationCandidate::from_responses_body(&next).unwrap();
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first_candidate,
        CodexContinuationResponse::from_response(
            &text_response("two"),
            Some("resp_1".into()),
            vec![json!({"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"two"}]})],
        ),
    );

    let plan = state
        .continuation_delta(&next_candidate)
        .expect("snapshot extends this turn");
    assert_eq!(plan["previous_response_id"], "resp_1");
    assert_eq!(
        plan["input"],
        json!([
            {"type":"configuration_update","reasoning":{"effort":"high"}},
            {"role":"user","content":[{"type":"input_text","text":"three"}]},
        ])
    );
}
