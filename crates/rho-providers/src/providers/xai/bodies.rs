//! xAI Responses create and compact request body builders.
//!
//! These endpoints do not share a field bundle: create uses the instructions
//! channel, tools, stream, and reasoning include; compact only accepts `model`
//! and a full `input` window (system messages included).

use crate::protocol::openai_responses::{codex_input_items_for_target, to_responses_lite_tool};
use serde_json::{json, Value};

use super::reasoning;
use crate::model::{Message, ModelError, ModelIdentity, ModelRequest};

/// Lowered fields used only by the Responses create body.
struct XaiCreateLowered {
    instructions: String,
    input: Vec<Value>,
    prompt_cache_key: Option<String>,
    reasoning_effort: Option<&'static str>,
}

fn lower_xai_create_request(
    provider: &'static str,
    model: &str,
    reasoning: &reasoning::XaiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<XaiCreateLowered, ModelError> {
    let mut instructions = Vec::new();
    let target = ModelIdentity::new(provider, "openai-responses", model);
    let input =
        codex_input_items_for_target(request.messages.to_vec(), &mut instructions, Some(&target))?;
    Ok(XaiCreateLowered {
        instructions: instructions.join("\n\n"),
        input,
        prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
        reasoning_effort: reasoning.effort(request.reasoning_level),
    })
}

/// Builds a streaming Responses create body for an xAI model turn.
///
/// Always requests encrypted reasoning content so later server-side compaction
/// can fold prior thinking into the opaque artifact.
pub(super) fn build_xai_responses_body(
    provider: &'static str,
    model: &str,
    reasoning: &reasoning::XaiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let tools = request
        .tools
        .iter()
        .cloned()
        .map(to_responses_lite_tool)
        .collect::<Vec<_>>();
    let XaiCreateLowered {
        instructions,
        input,
        prompt_cache_key,
        reasoning_effort,
    } = lower_xai_create_request(provider, model, reasoning, request)?;
    let mut body = json!({
        "model": model,
        "input": input,
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if let Some(prompt_cache_key) = prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning"] = json!({ "effort": effort });
    }
    Ok(body)
}

/// Builds a unary `/responses/compact` body.
///
/// xAI documents only `model` and `input`. System prompts must travel in
/// `input` (not an instructions channel). Create-only fields such as tools,
/// stream, include, reasoning, prompt_cache_key, and store are omitted.
pub(super) fn build_xai_compact_body(
    provider: &'static str,
    model: &str,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let target = ModelIdentity::new(provider, "openai-responses", model);
    let input = xai_compact_input_items(request.messages.to_vec(), &target)?;
    Ok(json!({
        "model": model,
        "input": input,
    }))
}

/// Converts history for compact: system messages stay in `input` in order.
fn xai_compact_input_items(
    messages: Vec<Message>,
    target: &ModelIdentity,
) -> Result<Vec<Value>, ModelError> {
    let mut input = Vec::new();
    for message in messages {
        match message {
            Message::System(content) => {
                input.push(json!({
                    "role": "system",
                    "content": content,
                }));
            }
            other => {
                let mut peeled = Vec::new();
                let items = codex_input_items_for_target(vec![other], &mut peeled, Some(target))?;
                debug_assert!(
                    peeled.is_empty(),
                    "non-system messages must not peel instructions"
                );
                input.extend(items);
            }
        }
    }
    Ok(input)
}
