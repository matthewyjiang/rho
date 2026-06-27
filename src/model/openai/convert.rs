use serde::Deserialize;
use serde_json::json;

use crate::model::{ContentBlock, Message, ModelError, ModelResponse};
use crate::tool::{ToolCall, ToolSpec};

use super::types::{
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
    let effort = effort.filter(|value| !value.eq_ignore_ascii_case("none"));
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

pub(crate) fn codex_input_items(
    messages: Vec<Message>,
    instructions: &mut Vec<String>,
) -> Result<Vec<serde_json::Value>, ModelError> {
    let mut input = Vec::new();
    for message in messages {
        match message {
            Message::System(content) => instructions.push(content),
            Message::User(blocks) => input.push(json!({
                "role": "user",
                "content": render_blocks(&blocks),
            })),
            Message::Assistant(blocks) => {
                let text = blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.as_str()),
                        ContentBlock::ToolCall(_) => None,
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

pub(crate) fn to_openai_message(message: Message) -> Result<OpenAiMessage, ModelError> {
    match message {
        Message::System(content) => Ok(openai_text_message("system", content)),
        Message::User(blocks) => Ok(openai_text_message("user", render_blocks(&blocks))),
        Message::Assistant(blocks) => {
            let content = blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    ContentBlock::ToolCall(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let tool_calls = blocks
                .into_iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolCall(call) => Some(tool_call_to_openai(call)),
                    ContentBlock::Text(_) => None,
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(OpenAiMessage {
                role: "assistant".into(),
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id: None,
            })
        }
        Message::ToolResult(result) => Ok(OpenAiMessage {
            role: "tool".into(),
            content: Some(result.content),
            tool_calls: None,
            tool_call_id: Some(result.id),
        }),
    }
}

fn openai_text_message(role: &str, content: String) -> OpenAiMessage {
    OpenAiMessage {
        role: role.into(),
        content: Some(content),
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

fn render_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.clone(),
            ContentBlock::ToolCall(call) => render_tool_call(call),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_call(call: &ToolCall) -> String {
    let arguments = serde_json::to_string_pretty(&call.arguments)
        .unwrap_or_else(|_| call.arguments.to_string());
    format!("Tool call: {}\n{}", call.name, arguments)
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
