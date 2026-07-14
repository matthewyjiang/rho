use serde::Deserialize;
use serde_json::json;

use crate::model::{ContentBlock, Message, ModelError, ModelResponse, PartialToolCall};
use crate::tool::{ToolCall, ToolSpec};

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
            Message::AbortedAssistant(mut message) => {
                message.content.extend(
                    message
                        .tool_calls
                        .iter()
                        .map(|call| ContentBlock::Text(render_partial_tool_call(call))),
                );
                message
                    .content
                    .push(ContentBlock::Text("[Operation aborted]".into()));
                append_codex_assistant(&mut input, message.content)?;
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

pub(crate) fn to_openai_message(message: Message) -> Result<OpenAiMessage, ModelError> {
    match message {
        Message::System(content) => Ok(openai_text_message("system", content)),
        Message::User(blocks) => Ok(OpenAiMessage {
            role: "user".into(),
            content: Some(chat_content_blocks(&blocks)),
            tool_calls: None,
            tool_call_id: None,
        }),
        Message::Assistant(blocks) => openai_assistant_message(blocks),
        Message::AbortedAssistant(mut message) => {
            message
                .content
                .push(ContentBlock::Text("[Operation aborted]".into()));
            openai_assistant_message(message.content)
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
