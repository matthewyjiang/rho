//! xAI Responses create and compact request body builders.
//!
//! These endpoints do not share a field bundle: create uses the instructions
//! channel, tools, stream, and reasoning include; compact only accepts `model`
//! and a full `input` window (system messages included).

use rho_sdk::model::ToolSpec;
use serde_json::{json, Value};

use crate::protocol::openai_responses::{
    codex_input_items_for_target, to_responses_lite_tool, ToolStrictness,
};

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
    let input = codex_input_items_for_target(request.messages, &mut instructions, Some(&target))?;
    Ok(XaiCreateLowered {
        instructions: instructions.join("\n\n"),
        input,
        prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
        reasoning_effort: reasoning.effort(request.reasoning_level),
    })
}

/// Serializes one xAI Responses tool, optionally rewriting web_search to the hosted type.
fn to_xai_responses_tool(
    tool: ToolSpec,
    strictness: ToolStrictness,
    hosted_web_search: bool,
) -> Value {
    if hosted_web_search && tool.name == "web_search" {
        return json!({
            "type": "web_search",
        });
    }
    to_responses_lite_tool(tool, strictness)
}

/// Maps client tool specs onto the xAI Responses tools array.
///
/// Hosted `x_search` and `image_generation` are xAI server-side amenities,
/// independent of client tools. `x_search` is attached on every create turn.
/// `image_generation` is attached when the host enables it. Both disappear as
/// soon as the session switches away from xAI. Stock Rho never registers
/// client tools with those names. A colliding custom `x_search` is always
/// dropped. A colliding custom `image_generation` is dropped only while the
/// hosted tool is advertised.
fn xai_responses_tools(
    tools: &[ToolSpec],
    hosted_web_search: bool,
    hosted_image_generation: bool,
) -> Vec<Value> {
    let mut out = tools
        .iter()
        .filter(|tool| {
            tool.name != "x_search" && !(hosted_image_generation && tool.name == "image_generation")
        })
        .cloned()
        .map(|tool| to_xai_responses_tool(tool, ToolStrictness::Explicit(false), hosted_web_search))
        .collect::<Vec<_>>();
    out.push(json!({ "type": "x_search" }));
    if hosted_image_generation {
        out.push(json!({ "type": "image_generation" }));
    }
    out
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
    hosted_web_search: bool,
    hosted_image_generation: bool,
) -> Result<Value, ModelError> {
    let tools = xai_responses_tools(request.tools, hosted_web_search, hosted_image_generation);
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
    // Hosted x_search is always present. Image generation is host-gated.
    body["tools"] = json!(tools);
    body["tool_choice"] = json!("auto");
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
    let input = xai_compact_input_items(request.messages, &target)?;
    Ok(json!({
        "model": model,
        "input": input,
    }))
}

/// Converts history for compact: system messages stay in `input` in order.
fn xai_compact_input_items(
    messages: &[Message],
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
                let items = codex_input_items_for_target(
                    std::slice::from_ref(other),
                    &mut peeled,
                    Some(target),
                )?;
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
