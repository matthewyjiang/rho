use serde::Deserialize;
use serde_json::json;

use crate::model::{
    handoff::{prepare_assistant, PreparedAssistant},
    ContentBlock, Message, ModelError, ModelResponse, PartialToolCall, ProviderContextBlock,
};
use rho_tools::tool::{ToolCall, ToolSpec};

use crate::protocol::openai_chat::{
    ChatResponse, OpenAiFunctionCall, OpenAiMessage, OpenAiTool, OpenAiToolCall, OpenAiToolFunction,
};

pub(crate) fn convert_openai_response(response: ChatResponse) -> Result<ModelResponse, ModelError> {
    let message = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ModelError::InvalidResponse("missing choices".into()))?
        .message;
    let mut blocks = Vec::new();
    if let Some(content) = message.content.filter(|s| !s.is_empty()) {
        blocks.push(ContentBlock::Text(content));
    }
    for call in message.tool_calls.unwrap_or_default() {
        let arguments = serde_json::from_str(&call.function.arguments).map_err(|e| {
            ModelError::InvalidResponse(format!(
                "invalid tool call arguments for {}: {e}",
                call.function.name
            ))
        })?;
        blocks.push(ContentBlock::ToolCall(ToolCall {
            id: call.id,
            name: call.function.name,
            arguments,
        }));
    }
    if blocks.is_empty() {
        Err(ModelError::InvalidResponse(
            "assistant message had no content or tool calls".into(),
        ))
    } else {
        Ok(ModelResponse::Assistant(blocks))
    }
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

pub(crate) fn to_responses_tool(tool: ToolSpec) -> serde_json::Value {
    if tool.name == "web_search" {
        return json!({
            "type": "web_search",
            "external_web_access": true,
        });
    }

    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
        "strict": false,
    })
}

pub(crate) fn to_responses_lite_tool(tool: ToolSpec) -> serde_json::Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
        "strict": false,
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
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::ToolCall(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
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
    let content = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::ToolCall(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tool_calls = blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(tool_call_to_openai(call)),
            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(OpenAiMessage {
        role: "assistant".into(),
        content: (!content.is_empty()).then(|| json!(content)),
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
            tool_calls: None,
            tool_call_id: None,
        }),
        Message::Assistant(blocks) => openai_assistant_message(blocks),
        Message::EnrichedAssistant(message) => {
            let fallback_target = message.provenance.clone().unwrap_or_else(|| {
                crate::model::ModelIdentity::new("foreign", "openai-chat-completions", "foreign")
            });
            openai_assistant_message(
                prepare_assistant(*message, target.unwrap_or(&fallback_target)).content,
            )
        }
        Message::AbortedAssistant(message) => {
            let fallback_target = message.provenance.clone().unwrap_or_else(|| {
                crate::model::ModelIdentity::new("foreign", "openai-chat-completions", "foreign")
            });
            let mut enriched = crate::model::AssistantMessage {
                content: aborted_content_as_non_executable(&message),
                provenance: message.provenance,
                reasoning_summary: message.reasoning_summary,
                provider_context: message.provider_context,
            };
            enriched
                .content
                .push(ContentBlock::Text("[Operation aborted]".into()));
            openai_assistant_message(
                prepare_assistant(enriched, target.unwrap_or(&fallback_target)).content,
            )
        }
        Message::ToolResult(result) => Ok(OpenAiMessage {
            role: "tool".into(),
            content: Some(json!(result.content)),
            tool_calls: None,
            tool_call_id: Some(result.id),
        }),
    }
}

fn openai_text_message(role: &str, content: String) -> OpenAiMessage {
    OpenAiMessage {
        role: role.into(),
        content: Some(json!(content)),
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
mod handoff_tests {
    use super::*;

    #[test]
    fn chat_handoff_keeps_foreign_reasoning_summary_as_tagged_text() {
        let source =
            crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
        let target =
            crate::model::ModelIdentity::new("openai", "openai-chat-completions", "gpt-chat-test");
        let message = Message::assistant(crate::model::AssistantMessage {
            content: vec![ContentBlock::Text("answer".into())],
            provenance: Some(source),
            reasoning_summary: Some("verified it".into()),
            provider_context: Vec::new(),
        });

        let converted = to_openai_message_for_target(message, Some(&target)).unwrap();
        let content = converted.content.unwrap().as_str().unwrap().to_string();

        assert!(content.contains("answer"));
        assert!(content.contains("<reasoning_summary>"));
        assert!(content.contains("verified it"));
    }

    #[test]
    fn codex_handoff_restores_replay_item_position() {
        let source =
            crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
        let message = Message::assistant(crate::model::AssistantMessage {
            content: vec![
                ContentBlock::Text("answer".into()),
                ContentBlock::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: json!({"command": "pwd"}),
                }),
            ],
            provenance: Some(source.clone()),
            reasoning_summary: None,
            provider_context: vec![crate::model::ProviderContextBlock {
                identity: source.clone(),
                kind: "openai_response_output_item".into(),
                position: Some(0),
                data: json!({"type": "reasoning", "encrypted_content": "signed"}),
            }],
        });

        let input =
            codex_input_items_for_target(vec![message], &mut Vec::new(), Some(&source)).unwrap();

        assert_eq!(input[0]["type"], "reasoning");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[2]["type"], "function_call");
    }

    #[test]
    fn codex_handoff_replays_only_exact_model_context() {
        let source =
            crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
        let message = Message::assistant(crate::model::AssistantMessage {
            content: vec![ContentBlock::Text("answer".into())],
            provenance: Some(source.clone()),
            reasoning_summary: Some("verified it".into()),
            provider_context: vec![crate::model::ProviderContextBlock {
                identity: source.clone(),
                kind: "openai_response_output_item".into(),
                position: None,
                data: json!({"type": "reasoning", "encrypted_content": "signed"}),
            }],
        });

        let exact =
            codex_input_items_for_target(vec![message.clone()], &mut Vec::new(), Some(&source))
                .unwrap();
        let foreign = codex_input_items_for_target(
            vec![message],
            &mut Vec::new(),
            Some(&crate::model::ModelIdentity::new(
                "anthropic",
                "anthropic-messages",
                "claude-test",
            )),
        )
        .unwrap();

        assert!(exact
            .iter()
            .any(|item| item["encrypted_content"] == "signed"));
        assert!(!foreign
            .iter()
            .any(|item| item["encrypted_content"] == "signed"));
        assert!(foreign.iter().any(|item| {
            item.get("content")
                .and_then(|content| content.as_str())
                .is_some_and(|content| content.contains("<reasoning_summary>"))
        }));
    }
}
