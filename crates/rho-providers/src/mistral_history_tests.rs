use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use crate::protocol::openai_chat::OpenAiMessage;

fn assistant(content: Option<Value>, reasoning: Option<&str>) -> OpenAiMessage {
    OpenAiMessage {
        role: "assistant".into(),
        content,
        reasoning_content: reasoning.map(str::to_owned),
        tool_calls: None,
        tool_call_id: None,
    }
}

fn normalized(mut messages: Vec<OpenAiMessage>) -> Value {
    super::normalize(&mut messages);
    serde_json::to_value(messages).unwrap()
}

// Covers: replayed same-model reasoning must reach Mistral as a thinking chunk;
// the undeclared `reasoning_content` field gets `422 extra_forbidden`
// Owner: Mistral chat history rewrite
#[test]
fn replays_reasoning_as_leading_thinking_chunk() {
    let thinking = json!({"type": "thinking", "thinking": [{"type": "text", "text": "plan"}]});
    for (content, expected) in [
        (
            Some(json!("answer")),
            json!([thinking, {"type": "text", "text": "answer"}]),
        ),
        (None, json!([thinking])),
        (Some(json!("")), json!([thinking])),
    ] {
        let body = normalized(vec![assistant(content, Some("plan"))]);
        assert_eq!(body[0], json!({"role": "assistant", "content": expected}));
    }
}
