use futures_util::StreamExt;

use crate::model::{ContentBlock, ModelError, ModelEvent, ModelResponse, ModelUsage};
use crate::tool::ToolCall;

use super::convert::{extract_response_text, ResponsesResponse};

pub(crate) fn trim_sse_line_end(line: &mut Vec<u8>) {
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
}

#[derive(Default)]
pub(crate) struct StreamedToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

pub(crate) fn handle_openai_stream_line(
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
    if let Some(usage) = extract_usage(&value) {
        on_event(ModelEvent::Usage(usage))?;
    }
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

pub(crate) fn convert_streamed_response(
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

pub(crate) struct CodexSseResponse {
    pub(crate) response: ModelResponse,
    pub(crate) response_id: Option<String>,
}

pub(crate) async fn collect_codex_sse_response(
    response: reqwest::Response,
    on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
) -> Result<CodexSseResponse, ModelError> {
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
pub(crate) struct CodexSseState {
    pub(crate) text: String,
    pub(crate) completed_text: Option<String>,
    pub(crate) tool_calls: Vec<ToolCall>,
    pub(crate) response_id: Option<String>,
}

impl CodexSseState {
    pub(crate) fn into_response(self) -> Result<CodexSseResponse, ModelError> {
        let response_id = self.response_id;
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
            Ok(CodexSseResponse {
                response: ModelResponse::Assistant(blocks),
                response_id,
            })
        }
    }
}

fn extract_codex_web_search_detail(item: &serde_json::Value) -> Option<String> {
    if item.get("type").and_then(|v| v.as_str()) != Some("web_search_call") {
        return None;
    }
    let action = item.get("action")?;
    if let Some(query) = action
        .get("query")
        .and_then(|query| query.as_str())
        .filter(|query| !query.is_empty())
    {
        return Some(format!("for \"{}\"", truncate_detail(query, 80)));
    }
    if let Some(queries) = action.get("queries").and_then(|queries| queries.as_array()) {
        let mut rendered = queries
            .iter()
            .filter_map(|query| query.as_str())
            .filter(|query| !query.is_empty())
            .take(3)
            .map(|query| format!("\"{}\"", truncate_detail(query, 48)))
            .collect::<Vec<_>>();
        if queries.len() > rendered.len() {
            rendered.push(format!("{} more", queries.len() - rendered.len()));
        }
        if !rendered.is_empty() {
            return Some(format!("for {}", rendered.join(", ")));
        }
    }
    if let Some(url) = action
        .get("url")
        .and_then(|url| url.as_str())
        .filter(|url| !url.is_empty())
    {
        return Some(format!("opened {}", truncate_detail(url, 80)));
    }
    if let Some(pattern) = action
        .get("pattern")
        .and_then(|pattern| pattern.as_str())
        .filter(|pattern| !pattern.is_empty())
    {
        return Some(format!("found \"{}\"", truncate_detail(pattern, 80)));
    }
    Some("finished".into())
}

fn truncate_detail(value: &str, max_chars: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max_chars {
        return value;
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
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

pub(crate) fn handle_codex_sse_line(
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
        let item = value.get("item").unwrap_or(&value);
        if let Some(detail) = extract_codex_web_search_detail(item) {
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::WebSearch(detail))?;
            }
        }
        if let Some(call) = extract_codex_function_call(item)? {
            state.tool_calls.push(call);
        }
    } else if event_type == "response.completed" {
        if let Some(response_id) = value
            .get("response")
            .and_then(|response| response.get("id"))
            .or_else(|| value.get("id"))
            .and_then(|id| id.as_str())
        {
            state.response_id = Some(response_id.to_string());
        }
        if let Some(usage) = value
            .get("response")
            .and_then(extract_usage)
            .or_else(|| extract_usage(&value))
        {
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::Usage(usage))?;
            }
        }
        if !state.text.is_empty() || !state.tool_calls.is_empty() {
            return Ok(());
        }
        if let Some(output) = value
            .get("response")
            .and_then(|response| response.get("output"))
            .and_then(|output| output.as_array())
        {
            for item in output {
                if let Some(detail) = extract_codex_web_search_detail(item) {
                    if let Some(on_event) = on_event.as_mut() {
                        on_event(ModelEvent::WebSearch(detail))?;
                    }
                }
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

fn extract_usage(value: &serde_json::Value) -> Option<ModelUsage> {
    let usage = value.get("usage")?;
    let raw_input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64());
    let total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64());
    let input_details = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"));
    let cache_read_tokens = input_details
        .and_then(|v| {
            v.get("cached_tokens")
                .or_else(|| v.get("cache_read_tokens"))
                .or_else(|| v.get("cached_input_tokens"))
        })
        .and_then(|v| v.as_u64());
    let cache_write_tokens = input_details
        .and_then(|v| {
            v.get("cache_write_tokens")
                .or_else(|| v.get("cache_creation_input_tokens"))
                .or_else(|| v.get("cache_creation_tokens"))
        })
        .and_then(|v| v.as_u64());
    let context_window = usage
        .get("context_window")
        .or_else(|| usage.get("context_window_tokens"))
        .and_then(|v| v.as_u64());
    let cost_usd_micros = usage
        .get("cost_usd")
        .or_else(|| usage.get("estimated_cost_usd"))
        .or_else(|| usage.get("cost"))
        .or_else(|| usage.get("estimated_cost"))
        .and_then(parse_usd_micros);

    let input_tokens = match (raw_input_tokens, cache_read_tokens) {
        (Some(input), Some(cached)) => Some(input.saturating_sub(cached)),
        (input, None) => input,
        (None, Some(_)) => None,
    };

    Some(ModelUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens,
        context_window,
        cost_usd_micros,
    })
}

fn parse_usd_micros(value: &serde_json::Value) -> Option<u64> {
    let dollars = value.as_f64().or_else(|| {
        value
            .as_str()?
            .trim_start_matches('$')
            .replace(',', "")
            .parse()
            .ok()
    })?;
    dollars
        .is_finite()
        .then(|| (dollars.max(0.0) * 1_000_000.0).round() as u64)
}

#[cfg(test)]
pub(crate) fn extract_sse_text(body: &str) -> Result<String, ModelError> {
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
