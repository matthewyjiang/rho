use futures_util::StreamExt;

use crate::model::{ContentBlock, ModelError, ModelEvent, ModelResponse};
use crate::tool::ToolCall;

use super::convert::{extract_response_text, ResponsesResponse};

pub(super) fn trim_sse_line_end(line: &mut Vec<u8>) {
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
}

#[derive(Default)]
pub(super) struct StreamedToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

pub(super) fn handle_openai_stream_line(
    line: &str,
    text: &mut String,
    tool_calls: &mut Vec<StreamedToolCall>,
    on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
) -> Result<(), ModelError> {
    let Some(data) = line.strip_prefix("data: ") else {
        return Ok(());
    };
    if data == "[DONE]" {
        return Ok(());
    }
    let Some(value) = serde_json::from_str::<serde_json::Value>(data).ok() else {
        return Ok(());
    };
    let Some(choice) = value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|choices| choices.first())
    else {
        return Ok(());
    };
    let delta = choice.get("delta");
    if let Some(reasoning_delta) = delta
        .and_then(|v| {
            v.get("reasoning_content")
                .or_else(|| v.get("reasoning"))
                .or_else(|| v.get("reasoning_text"))
        })
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        on_event(ModelEvent::ReasoningDelta(reasoning_delta.to_string()))?;
    }
    if let Some(content_delta) = delta
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        on_event(ModelEvent::OutputDelta(content_delta.to_string()))?;
        text.push_str(content_delta);
    }
    let Some(delta_tool_calls) = delta
        .and_then(|v| v.get("tool_calls"))
        .and_then(|v| v.as_array())
    else {
        return Ok(());
    };

    for delta in delta_tool_calls {
        let index = delta
            .get("index")
            .and_then(|v| v.as_u64())
            .unwrap_or(tool_calls.len() as u64) as usize;
        while tool_calls.len() <= index {
            tool_calls.push(StreamedToolCall::default());
        }
        let call = &mut tool_calls[index];
        if let Some(id) = delta.get("id").and_then(|v| v.as_str()) {
            call.id = Some(id.to_string());
        }
        if let Some(name) = delta
            .get("function")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
        {
            call.name = Some(name.to_string());
        }
        if let Some(arguments) = delta
            .get("function")
            .and_then(|v| v.get("arguments"))
            .and_then(|v| v.as_str())
        {
            call.arguments.push_str(arguments);
        }
    }
    Ok(())
}

pub(super) fn convert_streamed_response(
    text: String,
    tool_calls: Vec<StreamedToolCall>,
) -> Result<ModelResponse, ModelError> {
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(ContentBlock::Text(text));
    }
    for (index, call) in tool_calls.into_iter().enumerate() {
        let id = call.id.ok_or_else(|| {
            ModelError::InvalidResponse(format!("streamed tool call {index} missing id"))
        })?;
        let name = call.name.ok_or_else(|| {
            ModelError::InvalidResponse(format!("streamed tool call {index} missing name"))
        })?;
        let arguments = serde_json::from_str(&call.arguments).map_err(|e| {
            ModelError::InvalidResponse(format!("invalid tool call arguments for {name}: {e}"))
        })?;
        blocks.push(ContentBlock::ToolCall(ToolCall {
            id,
            name,
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

pub(super) async fn collect_codex_sse_response(
    response: reqwest::Response,
    on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
) -> Result<ModelResponse, ModelError> {
    let mut state = CodexSseState::default();
    let mut buffer = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);
        while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = buffer.drain(..=newline).collect::<Vec<_>>();
            trim_sse_line_end(&mut line);
            let line = std::str::from_utf8(&line).map_err(|err| {
                ModelError::InvalidResponse(format!(
                    "streamed response contained invalid utf-8: {err}"
                ))
            })?;
            handle_codex_sse_line(line, &mut state, on_event)?;
        }
    }
    if !buffer.is_empty() {
        trim_sse_line_end(&mut buffer);
        let line = std::str::from_utf8(&buffer).map_err(|err| {
            ModelError::InvalidResponse(format!("streamed response contained invalid utf-8: {err}"))
        })?;
        handle_codex_sse_line(line, &mut state, on_event)?;
    }
    state.into_response()
}

fn sse_data(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

fn extract_reasoning_delta(value: &serde_json::Value) -> Option<String> {
    for key in [
        "delta",
        "text",
        "content",
        "summary",
        "reasoning",
        "reasoning_text",
    ] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }
    for key in ["delta", "text", "content", "summary"] {
        if let Some(text) = value
            .get("item")
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
        {
            return Some(text.to_string());
        }
    }
    None
}

#[derive(Default)]
pub(super) struct CodexSseState {
    pub(super) text: String,
    pub(super) completed_text: Option<String>,
    pub(super) tool_calls: Vec<ToolCall>,
}

impl CodexSseState {
    pub(super) fn into_response(self) -> Result<ModelResponse, ModelError> {
        let mut blocks = Vec::new();
        let text = if self.text.is_empty() {
            self.completed_text.unwrap_or_default()
        } else {
            self.text
        };
        if !text.is_empty() {
            blocks.push(ContentBlock::Text(text));
        }
        blocks.extend(self.tool_calls.into_iter().map(ContentBlock::ToolCall));
        if blocks.is_empty() {
            Err(ModelError::InvalidResponse(
                "missing response content in SSE".into(),
            ))
        } else {
            Ok(ModelResponse::Assistant(blocks))
        }
    }
}

fn extract_codex_function_call(item: &serde_json::Value) -> Result<Option<ToolCall>, ModelError> {
    if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
        return Ok(None);
    }
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ModelError::InvalidResponse("function_call missing name".into()))?
        .to_string();
    let id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ModelError::InvalidResponse(format!("function_call {name} missing call_id"))
        })?
        .to_string();
    let arguments_text = item
        .get("arguments")
        .and_then(|v| v.as_str())
        .unwrap_or("{}");
    let arguments = serde_json::from_str(arguments_text).map_err(|e| {
        ModelError::InvalidResponse(format!("invalid function_call arguments for {name}: {e}"))
    })?;
    Ok(Some(ToolCall {
        id,
        name,
        arguments,
    }))
}

pub(super) fn handle_codex_sse_line(
    line: &str,
    state: &mut CodexSseState,
    on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
) -> Result<(), ModelError> {
    let Some(data) = sse_data(line) else {
        return Ok(());
    };
    if data == "[DONE]" {
        return Ok(());
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return Ok(());
    };
    let event_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if event_type == "response.output_text.delta" {
        if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
            state.text.push_str(delta);
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::OutputDelta(delta.to_string()))?;
            }
        }
    } else if event_type.contains("reasoning") && event_type.ends_with(".delta") {
        if let Some(delta) = extract_reasoning_delta(&value) {
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::ReasoningDelta(delta))?;
            }
        }
    } else if event_type == "response.output_item.done" {
        if let Some(call) = extract_codex_function_call(value.get("item").unwrap_or(&value))? {
            state.tool_calls.push(call);
        }
    } else if event_type == "response.completed"
        && state.text.is_empty()
        && state.tool_calls.is_empty()
    {
        if let Some(output) = value
            .get("response")
            .and_then(|response| response.get("output"))
            .and_then(|output| output.as_array())
        {
            for item in output {
                if let Some(call) = extract_codex_function_call(item)? {
                    state.tool_calls.push(call);
                }
            }
        }
        if state.tool_calls.is_empty() {
            if let Ok(response) =
                serde_json::from_value::<ResponsesResponse>(value["response"].clone())
            {
                state.completed_text = Some(extract_response_text(response)?);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn extract_sse_text(body: &str) -> Result<String, ModelError> {
    let mut text = String::new();
    for line in body.lines() {
        let Some(data) = sse_data(line) else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        match value.get("type").and_then(|v| v.as_str()) {
            Some("response.output_text.delta") => {
                if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                    text.push_str(delta);
                }
            }
            Some("response.completed") if text.is_empty() => {
                if let Ok(response) =
                    serde_json::from_value::<ResponsesResponse>(value["response"].clone())
                {
                    return extract_response_text(response);
                }
            }
            _ => {}
        }
    }
    if text.is_empty() {
        Err(ModelError::InvalidResponse(format!(
            "missing response text in SSE: {body}"
        )))
    } else {
        Ok(text)
    }
}
