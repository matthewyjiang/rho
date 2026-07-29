//! xAI Responses create and compact request body builders.
//!
//! These endpoints do not share a field bundle: create uses the instructions
//! channel, tools, stream, and reasoning include; compact only accepts `model`
//! and a full `input` window (system messages included).

use rho_sdk::model::ToolSpec;
use serde_json::{json, Value};

use crate::protocol::openai_responses::{
    codex_input_items_for_target, to_xai_responses_tool, ToolStrictness,
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
    let input =
        codex_input_items_for_target(request.messages.to_vec(), &mut instructions, Some(&target))?;
    Ok(XaiCreateLowered {
        instructions: instructions.join("\n\n"),
        input,
        prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
        reasoning_effort: reasoning.effort(request.reasoning_level),
    })
}

/// Maps client tool specs onto the xAI Responses tools array.
///
/// `x_search` is an xAI-hosted server-side tool for searching X (x.com). It is
/// independent of the client `web_search` tool and is attached as a provider
/// amenity on every xAI create turn, including when the client tool list is
/// empty or restricted. It disappears as soon as the session switches away
/// from xAI. Stock Rho never registers a client tool named `x_search`; any
/// colliding custom function of that name is dropped so only the hosted form
/// is advertised.
fn xai_responses_tools(tools: &[ToolSpec], hosted_web_search: bool) -> Vec<Value> {
    let mut out = tools
        .iter()
        .filter(|tool| tool.name != "x_search")
        .cloned()
        .map(|tool| to_xai_responses_tool(tool, ToolStrictness::Explicit(false), hosted_web_search))
        .collect::<Vec<_>>();
    out.push(json!({ "type": "x_search" }));
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
) -> Result<Value, ModelError> {
    let tools = xai_responses_tools(request.tools, hosted_web_search);
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
    // Hosted x_search is always present on xAI create turns.
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
