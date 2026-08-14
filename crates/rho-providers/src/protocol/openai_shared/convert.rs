use serde::Deserialize;
use serde_json::json;

use crate::model::{
    handoff::{prepare_assistant, PreparedAssistant},
    ContentBlock, Message, ModelError, ModelResponse, PartialToolCall, ProviderContextBlock,
};
use rho_sdk::model::{ToolCall, ToolSpec};

use crate::protocol::openai_chat::{
    ChatResponse, OpenAiFunctionCall, OpenAiMessage, OpenAiTool, OpenAiToolCall, OpenAiToolFunction,
};

use super::tool_calls::{finalize_chat_tool_calls, ChatToolCallPolicy, RawChatToolCall};

/// Provider-context kind for chat-completions `reasoning_content`.
///
/// Models such as Qwen3.x stream thinking in `delta.reasoning_content` and expect
/// that field to be replayed on later assistant messages that carried tool calls.
/// Raw reasoning stays opaque provider context (never `reasoning_summary`).
pub(crate) const OPENAI_CHAT_REASONING_CONTENT_KIND: &str = "openai_chat_reasoning_content";

/// Normalized chat-completions assistant payload after stream or JSON convert.
#[derive(Debug)]
pub(crate) struct ChatAssistantFinish {
    pub(crate) response: ModelResponse,
    pub(crate) reasoning_content: Option<String>,
}

/// Builds the shared chat assistant response from accumulated text, reasoning,
/// and tool calls. Callers emit [`crate::model::ModelEvent::ProviderContext`]
/// for reasoning separately via [`emit_chat_reasoning_context`].
pub(crate) fn finalize_chat_assistant(
    text: String,
    reasoning: String,
    tool_calls: Vec<RawChatToolCall>,
    policy: ChatToolCallPolicy,
) -> Result<ChatAssistantFinish, ModelError> {
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(ContentBlock::Text(text));
    }
    blocks.extend(
        finalize_chat_tool_calls(tool_calls, policy)?
            .into_iter()
            .map(ContentBlock::ToolCall),
    );
    if blocks.is_empty() {
        return Err(ModelError::empty_assistant());
    }
    Ok(ChatAssistantFinish {
        response: ModelResponse::Assistant(blocks),
        reasoning_content: (!reasoning.is_empty()).then_some(reasoning),
    })
}

/// Publishes retained chat `reasoning_content` as opaque provider context.
pub(crate) fn emit_chat_reasoning_context(
    reasoning_content: Option<String>,
    on_event: &mut (dyn FnMut(crate::model::ModelEvent) -> Result<(), ModelError> + Send),
) -> Result<(), ModelError> {
    let Some(reasoning) = reasoning_content.filter(|text| !text.is_empty()) else {
        return Ok(());
    };
    on_event(crate::model::ModelEvent::ProviderContext {
        kind: OPENAI_CHAT_REASONING_CONTENT_KIND.into(),
        position: Some(0),
        data: serde_json::Value::String(reasoning),
    })
}

/// Non-stream completions keep only the portable assistant response.
///
/// Chat `reasoning_content` is stream-path provider context
/// ([`OPENAI_CHAT_REASONING_CONTENT_KIND`]). `send_turn` / `complete_turn` have
/// no event channel to publish it, and [`ModelResponse`] cannot carry it.
/// Rho orchestration always uses the stream path for multi-step tool loops.
pub(crate) fn response_without_stream_context(finish: ChatAssistantFinish) -> ModelResponse {
    let _ = finish.reasoning_content;
    finish.response
}

/// Converts a non-stream chat completion using the shared assistant finalizer.
///
/// Reasoning is returned on [`ChatAssistantFinish`] so callers can emit
/// provider context on event-capable paths.
pub(crate) fn convert_openai_response(
    response: ChatResponse,
    policy: ChatToolCallPolicy,
) -> Result<ChatAssistantFinish, ModelError> {
    let message = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ModelError::InvalidResponse("missing choices".into()))?
        .message;
    let text = message
        .content
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    let reasoning = message
        .reasoning_content
        .filter(|text| !text.is_empty())
        .unwrap_or_default();
    let raw_calls = message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|call| RawChatToolCall {
            id: Some(call.id),
            name: Some(call.function.name),
            arguments: call.function.arguments,
        })
        .collect();
    finalize_chat_assistant(text, reasoning, raw_calls, policy)
}

/// Known chat wire fields extracted from replayable provider context.
struct ChatReplay {
    reasoning_content: Option<String>,
}

fn chat_replay(replay_context: Vec<ProviderContextBlock>) -> Result<ChatReplay, ModelError> {
    let mut reasoning_content = None;
    let mut unknown_kinds = Vec::new();
    for block in replay_context {
        if block.kind == OPENAI_CHAT_REASONING_CONTENT_KIND {
            let Some(text) = block
                .data
                .as_str()
                .map(str::to_owned)
                .filter(|text| !text.is_empty())
            else {
                continue;
            };
            if reasoning_content.is_some() {
                return Err(ModelError::InvalidResponse(
                    "openai chat received multiple reasoning_content replay blocks".into(),
                ));
            }
            reasoning_content = Some(text);
            continue;
        }
        if !unknown_kinds.contains(&block.kind) {
            unknown_kinds.push(block.kind);
        }
    }
    if !unknown_kinds.is_empty() {
        return Err(ModelError::InvalidResponse(format!(
            "openai chat cannot encode provider context kinds: {}",
            unknown_kinds.join(", ")
        )));
    }
    Ok(ChatReplay { reasoning_content })
}

pub(crate) fn codex_reasoning_param(
    effort: Option<&str>,
    summary: Option<&str>,
) -> Option<serde_json::Value> {
    let summary = summary.filter(|value| !value.eq_ignore_ascii_case("none"));
    if effort.is_none() && summary.is_none() {
        return None;
    }
    let mut reasoning = serde_json::Map::new();
    if let Some(effort) = effort {
        reasoning.insert("effort".into(), json!(effort));
    }
    if let Some(summary) = summary {
        reasoning.insert("summary".into(), json!(summary));
    }
    Some(serde_json::Value::Object(reasoning))
}

pub(crate) fn to_openai_tool(tool: ToolSpec) -> OpenAiTool {
    OpenAiTool {
        kind: "function",
        function: OpenAiToolFunction {
            name: tool.name,
            description: tool.description,
            parameters: tool.input_schema,
            strict: false,
        },
    }
}

/// The `strict` value a Responses function tool carries on the wire.
///
/// Every Responses endpoint keeps the `strict` key, so this picks the value
/// rather than whether the field appears. Codex sends an explicit JSON `null`;
/// the public OpenAI API and xAI send a boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolStrictness {
    /// Serializes as JSON `null`.
    Unspecified,
    /// Serializes as JSON `true` or `false`.
    Explicit(bool),
}

impl ToolStrictness {
    fn to_json(self) -> serde_json::Value {
        match self {
            Self::Unspecified => serde_json::Value::Null,
            Self::Explicit(strict) => json!(strict),
        }
    }
}

/// Serializes a tool for a Responses endpoint that offers hosted tools.
pub(crate) fn to_responses_tool(
    tool: ToolSpec,
    strictness: ToolStrictness,
    hosted_web_search: bool,
) -> serde_json::Value {
    if hosted_web_search && tool.name == "web_search" {
        return json!({
            "type": "web_search",
            "external_web_access": true,
        });
    }

    to_responses_lite_tool(tool, strictness)
}

/// Serializes a tool for a Responses endpoint with no hosted tool types.
pub(crate) fn to_responses_lite_tool(
    tool: ToolSpec,
    strictness: ToolStrictness,
) -> serde_json::Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
        "strict": strictness.to_json(),
    })
}

pub(crate) fn codex_input_items(
    messages: Vec<Message>,
    instructions: &mut Vec<String>,
) -> Result<Vec<serde_json::Value>, ModelError> {
    codex_input_items_for_target(messages, instructions, None)
}

pub(crate) fn codex_input_items_for_target(
    messages: Vec<Message>,
    instructions: &mut Vec<String>,
    target: Option<&crate::model::ModelIdentity>,
) -> Result<Vec<serde_json::Value>, ModelError> {
    let mut input = Vec::new();
    for message in messages {
        match message {
            Message::System(content) => instructions.push(content),
            Message::User(blocks) => input.push(json!({
                "role": "user",
                "content": codex_content_blocks(&blocks),
            })),
            Message::Assistant(blocks) => {
                append_codex_assistant(&mut input, blocks)?;
            }
            Message::EnrichedAssistant(message) => {
                let fallback_target = message.provenance.clone().unwrap_or_else(|| {
                    crate::model::ModelIdentity::new("foreign", "openai-responses", "foreign")
                });
                let prepared = prepare_assistant(*message, target.unwrap_or(&fallback_target));
                append_codex_prepared_assistant(&mut input, prepared)?;
            }
            Message::AbortedAssistant(message) => {
                let mut enriched = crate::model::AssistantMessage {
                    content: aborted_content_as_non_executable(&message),
                    provenance: message.provenance,
                    reasoning_summary: message.reasoning_summary,
                    provider_context: message.provider_context,
                };
                enriched
                    .content
                    .push(ContentBlock::Text("[Operation aborted]".into()));
                let fallback_target = enriched.provenance.clone().unwrap_or_else(|| {
                    crate::model::ModelIdentity::new("foreign", "openai-responses", "foreign")
                });
                let prepared = prepare_assistant(enriched, target.unwrap_or(&fallback_target));
                append_codex_prepared_assistant(&mut input, prepared)?;
            }
            Message::ToolResult(result) => input.push(json!({
                "type": "function_call_output",
                "call_id": result.id,
                "output": result.content,
            })),
        }
    }
    Ok(input)
}

fn append_codex_prepared_assistant(
    input: &mut Vec<serde_json::Value>,
    prepared: PreparedAssistant,
) -> Result<(), ModelError> {
    let mut assistant_items = Vec::new();
    // `prepare_assistant` already suppresses portable fallback when opaque
    // context can replay, so converters only append the lowered content.
    append_codex_assistant(&mut assistant_items, prepared.content)?;
    insert_replay_items(&mut assistant_items, prepared.replay_context);
    input.extend(assistant_items);
    Ok(())
}

fn insert_replay_items(
    assistant_items: &mut Vec<serde_json::Value>,
    replay_context: Vec<ProviderContextBlock>,
) {
    let mut replay_items = replay_context
        .into_iter()
        .enumerate()
        .filter(|(_, block)| block.kind == "openai_response_output_item")
        .collect::<Vec<_>>();
    replay_items.sort_by_key(|(sequence, block)| (block.position.unwrap_or(usize::MAX), *sequence));
    let (positioned, unpositioned): (Vec<_>, Vec<_>) = replay_items
        .into_iter()
        .partition(|(_, block)| block.position.is_some());
    for (_, block) in positioned.into_iter().rev() {
        let position = block
            .position
            .expect("positioned replay item has a position")
            .min(assistant_items.len());
        assistant_items.insert(position, block.data);
    }
    assistant_items.extend(unpositioned.into_iter().map(|(_, block)| block.data));
}

fn append_codex_assistant(
    input: &mut Vec<serde_json::Value>,
    blocks: Vec<ContentBlock>,
) -> Result<(), ModelError> {
    let text = assistant_text(&blocks);
    if !text.is_empty() {
        input.push(json!({ "role": "assistant", "content": text }));
    }
    for block in blocks {
        if let ContentBlock::ToolCall(call) = block {
            input.push(json!({
                "type": "function_call",
                "call_id": call.id,
                "name": call.name,
                "arguments": serde_json::to_string(&call.arguments).map_err(|e| ModelError::InvalidResponse(format!("invalid tool call arguments: {e}")))?,
            }));
        }
    }
    Ok(())
}

fn openai_assistant_message(blocks: Vec<ContentBlock>) -> Result<OpenAiMessage, ModelError> {
    openai_prepared_assistant(
        PreparedAssistant {
            content: blocks,
            replay_context: Vec::new(),
        },
        // Plain assistant history carries no provenance, so it never gets
        // same-model reasoning synthesis.
        /*synthesize_tool_reasoning*/
        false,
    )
}

/// DeepSeek's thinking mode rejects tool-call turns that omit
/// `reasoning_content` (a present empty string is accepted), while stricter
/// chat hosts can reject the field as unknown. Synthesis is therefore scoped
/// to same-model turns for the models documented to require it.
fn synthesizes_tool_reasoning(
    provenance: Option<&crate::model::ModelIdentity>,
    target: &crate::model::ModelIdentity,
) -> bool {
    provenance == Some(target) && target.model.to_ascii_lowercase().contains("deepseek")
}

/// Converts a prepared assistant turn for the chat-completions wire.
///
/// `synthesize_tool_reasoning` makes tool-call turns always carry
/// `reasoning_content`, defaulting to an empty string; see
/// [`synthesizes_tool_reasoning`].
fn openai_prepared_assistant(
    prepared: PreparedAssistant,
    synthesize_tool_reasoning: bool,
) -> Result<OpenAiMessage, ModelError> {
    let replay = chat_replay(prepared.replay_context)?;
    let content = assistant_text(&prepared.content);
    let tool_calls = prepared
        .content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(tool_call_to_openai(call)),
            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(OpenAiMessage {
        role: "assistant".into(),
        content: (!content.is_empty()).then(|| json!(content)),
        reasoning_content: replay
            .reasoning_content
            .or_else(|| (synthesize_tool_reasoning && !tool_calls.is_empty()).then(String::new)),
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        tool_call_id: None,
    })
}

pub(crate) fn to_openai_message_for_target(
    message: Message,
    target: Option<&crate::model::ModelIdentity>,
) -> Result<OpenAiMessage, ModelError> {
    match message {
        Message::System(content) => Ok(openai_text_message("system", content)),
        Message::User(blocks) => Ok(OpenAiMessage {
            role: "user".into(),
            content: Some(chat_content_blocks(&blocks)),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }),
        Message::Assistant(blocks) => openai_assistant_message(blocks),
        Message::EnrichedAssistant(message) => {
            let fallback_target = message.provenance.clone().unwrap_or_else(|| {
                crate::model::ModelIdentity::new("foreign", "openai-chat-completions", "foreign")
            });
            let target = target.unwrap_or(&fallback_target);
            let synthesize = synthesizes_tool_reasoning(message.provenance.as_ref(), target);
            openai_prepared_assistant(prepare_assistant(*message, target), synthesize)
        }
        Message::AbortedAssistant(message) => {
            let mut enriched = crate::model::AssistantMessage {
                content: aborted_content_as_non_executable(&message),
                provenance: message.provenance,
                reasoning_summary: message.reasoning_summary,
                provider_context: message.provider_context,
            };
            enriched
                .content
                .push(ContentBlock::Text("[Operation aborted]".into()));
            let fallback_target = enriched.provenance.clone().unwrap_or_else(|| {
                crate::model::ModelIdentity::new("foreign", "openai-chat-completions", "foreign")
            });
            let target = target.unwrap_or(&fallback_target);
            let synthesize = synthesizes_tool_reasoning(enriched.provenance.as_ref(), target);
            openai_prepared_assistant(prepare_assistant(enriched, target), synthesize)
        }
        Message::ToolResult(result) => Ok(OpenAiMessage {
            role: "tool".into(),
            content: Some(json!(result.content)),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: Some(result.id),
        }),
    }
}

fn openai_text_message(role: &str, content: String) -> OpenAiMessage {
    OpenAiMessage {
        role: role.into(),
        content: Some(json!(content)),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
    }
}

fn tool_call_to_openai(call: ToolCall) -> Result<OpenAiToolCall, ModelError> {
    let arguments = serde_json::to_string(&call.arguments)
        .map_err(|e| ModelError::InvalidResponse(format!("invalid tool call arguments: {e}")))?;
    Ok(OpenAiToolCall {
        id: call.id,
        kind: "function".into(),
        function: OpenAiFunctionCall {
            name: call.name,
            arguments,
        },
    })
}

fn chat_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    let content = blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => json!({ "type": "text", "text": text }),
            ContentBlock::Image(image) => json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", image.mime_type, image.data) },
            }),
            ContentBlock::ToolCall(call) => {
                json!({ "type": "text", "text": render_tool_call(call) })
            }
        })
        .collect::<Vec<_>>();
    json!(content)
}

fn codex_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    let content = blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => json!({ "type": "input_text", "text": text }),
            ContentBlock::Image(image) => json!({
                "type": "input_image",
                "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
            }),
            ContentBlock::ToolCall(call) => {
                json!({ "type": "input_text", "text": render_tool_call(call) })
            }
        })
        .collect::<Vec<_>>();
    json!(content)
}

/// Placeholder for an assistant image neither OpenAI protocol can encode.
pub(crate) const ASSISTANT_IMAGE_OMITTED_TEXT: &str =
    "[image omitted: assistant history cannot carry image content]";

/// Joins assistant text, replacing images with [`ASSISTANT_IMAGE_OMITTED_TEXT`].
///
/// Neither OpenAI wire protocol has an assistant image slot. Images degrade to
/// text so history keeps a trace of the content instead of dropping it silently
/// or failing the whole turn.
fn assistant_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) => Some(ASSISTANT_IMAGE_OMITTED_TEXT),
            ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_call(call: &ToolCall) -> String {
    let arguments = serde_json::to_string_pretty(&call.arguments)
        .unwrap_or_else(|_| call.arguments.to_string());
    format!("Tool call: {}\n{}", call.name, arguments)
}

fn render_partial_tool_call(call: &PartialToolCall) -> String {
    format!(
        "[Partial tool call (not executed)]\nID: {}\nName: {}\nArguments:\n{}",
        call.id.as_deref().unwrap_or("[unknown]"),
        call.name.as_deref().unwrap_or("[unknown]"),
        call.arguments,
    )
}

fn aborted_content_as_non_executable(
    message: &crate::model::AbortedAssistant,
) -> Vec<ContentBlock> {
    let mut blocks = Vec::with_capacity(message.content.len() + message.tool_calls.len());
    let mut seen_ids = std::collections::HashSet::new();
    for block in &message.content {
        match block {
            ContentBlock::ToolCall(call) => {
                seen_ids.insert(call.id.clone());
                blocks.push(ContentBlock::Text(render_tool_call(call)));
            }
            other => blocks.push(other.clone()),
        }
    }
    for call in &message.tool_calls {
        if call
            .id
            .as_ref()
            .is_some_and(|id| !id.is_empty() && seen_ids.contains(id))
        {
            continue;
        }
        blocks.push(ContentBlock::Text(render_partial_tool_call(call)));
    }
    blocks
}

#[derive(Deserialize)]
pub(crate) struct ResponsesResponse {
    output_text: Option<String>,
    output: Option<Vec<ResponseOutput>>,
}

#[derive(Deserialize)]
struct ResponseOutput {
    content: Option<Vec<ResponseContent>>,
}

#[derive(Deserialize)]
struct ResponseContent {
    text: Option<String>,
    annotations: Option<Vec<ResponseAnnotation>>,
}

#[derive(Deserialize)]
struct ResponseAnnotation {
    #[serde(rename = "type")]
    kind: Option<String>,
    title: Option<String>,
    url: Option<String>,
}

pub(crate) fn extract_response_text(response: ResponsesResponse) -> Result<String, ModelError> {
    let mut content_texts = Vec::new();
    let mut citations = Vec::new();
    for content in response
        .output
        .unwrap_or_default()
        .into_iter()
        .flat_map(|o| o.content.unwrap_or_default())
    {
        if let Some(text) = content.text.filter(|text| !text.is_empty()) {
            content_texts.push(text);
        }
        for annotation in content.annotations.unwrap_or_default() {
            if annotation.kind.as_deref() == Some("url_citation") {
                if let Some(url) = annotation.url.filter(|url| !url.trim().is_empty()) {
                    citations.push((annotation.title, url));
                }
            }
        }
    }

    let mut text = response
        .output_text
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| content_texts.join("\n"));
    if text.is_empty() {
        return Err(ModelError::InvalidResponse("missing response text".into()));
    }
    append_response_citations(&mut text, citations);
    Ok(text)
}

fn append_response_citations(text: &mut String, citations: Vec<(Option<String>, String)>) {
    let mut seen = std::collections::HashSet::new();
    let citations = citations
        .into_iter()
        .filter(|(_, url)| seen.insert(url.clone()))
        .collect::<Vec<_>>();
    if citations.is_empty() {
        return;
    }
    text.push_str("\n\nSources:");
    for (title, url) in citations {
        let title = title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| url.clone());
        text.push_str(&format!("\n- {title}: {url}"));
    }
}

#[cfg(test)]
#[path = "convert_image_tests.rs"]
mod image_tests;

#[cfg(test)]
#[path = "convert_handoff_tests.rs"]
mod handoff_tests;

#[cfg(test)]
#[path = "convert_finalize_tests.rs"]
mod finalize_tests;
