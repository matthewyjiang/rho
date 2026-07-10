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

    let plan = state.continuation_body(&next, body(next.input.clone()));

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
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(crate::tool::ToolCall {
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

    let plan = state.continuation_body(&next, body(next.input.clone()));

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
    let response = ModelResponse::Assistant(vec![ContentBlock::ToolCall(crate::tool::ToolCall {
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

    let plan = state.continuation_body(&next, body(next.input.clone()));

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
    let full_body = body(next.input.clone());
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &text_response("two"),
            Some("resp_1".into()),
            Vec::new(),
        ),
    );

    assert_eq!(state.continuation_body(&next, full_body.clone()), full_body);
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

    assert_eq!(state.continuation_body(&next, next_body.clone()), next_body);
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
    let full_body = body(next.input.clone());
    let mut state = CodexContinuationState::default();
    state.record_success(
        &first,
        CodexContinuationResponse::from_response(
            &text_response("two"),
            Some("resp_1".into()),
            vec![json!({"type":"web_search_call","id":"search_1"})],
        ),
    );

    assert_eq!(state.continuation_body(&next, full_body.clone()), full_body);
}
