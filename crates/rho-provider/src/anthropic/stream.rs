use futures_util::StreamExt;

use crate::{ModelError, ModelEvent, ModelResponse};

use super::convert::{convert_content_blocks, usage_to_model_usage};
use super::types::{AnthropicContentBlock, AnthropicUsage};

pub(super) fn trim_sse_line_end(line: &mut Vec<u8>) {
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
}

#[derive(Default)]
pub(super) struct AnthropicSseState {
    blocks: Vec<StreamedBlock>,
    last_output_tokens: u64,
}

#[derive(Default)]
struct StreamedBlock {
    text: String,
    tool_id: Option<String>,
    tool_name: Option<String>,
    tool_input: String,
}

impl AnthropicSseState {
    fn ensure_block(&mut self, index: usize) -> &mut StreamedBlock {
        while self.blocks.len() <= index {
            self.blocks.push(StreamedBlock::default());
        }
        &mut self.blocks[index]
    }

    pub(super) fn into_response(self) -> Result<ModelResponse, ModelError> {
        let mut blocks = Vec::new();
        for (index, block) in self.blocks.into_iter().enumerate() {
            if !block.text.is_empty() {
                blocks.push(AnthropicContentBlock::Text {
                    text: block.text,
                    cache_control: None,
                });
            }
            if let Some(id) = block.tool_id {
                let name = block.tool_name.ok_or_else(|| {
                    ModelError::InvalidResponse(format!(
                        "streamed tool_use block {index} missing name"
                    ))
                })?;
                let input = if block.tool_input.trim().is_empty() {
                    serde_json::Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&block.tool_input).map_err(|err| {
                        ModelError::InvalidResponse(format!(
                            "invalid streamed tool_use input for {name}: {err}"
                        ))
                    })?
                };
                blocks.push(AnthropicContentBlock::ToolUse { id, name, input });
            }
        }
        convert_content_blocks(blocks)
    }
}

pub(super) async fn collect_anthropic_sse_response(
    response: reqwest::Response,
    on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
) -> Result<ModelResponse, ModelError> {
    let mut state = AnthropicSseState::default();
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
            handle_anthropic_stream_line(line, &mut state, on_event)?;
        }
    }
    if !buffer.is_empty() {
        trim_sse_line_end(&mut buffer);
        let line = std::str::from_utf8(&buffer).map_err(|err| {
            ModelError::InvalidResponse(format!("streamed response contained invalid utf-8: {err}"))
        })?;
        handle_anthropic_stream_line(line, &mut state, on_event)?;
    }
    state.into_response()
}

fn sse_data(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

pub(super) fn handle_anthropic_stream_line(
    line: &str,
    state: &mut AnthropicSseState,
    on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
) -> Result<(), ModelError> {
    let Some(data) = sse_data(line) else {
        return Ok(());
    };
    if data == "[DONE]" {
        return Ok(());
    }
    let value = serde_json::from_str::<serde_json::Value>(data).map_err(|err| {
        ModelError::InvalidResponse(format!("invalid Anthropic stream JSON: {err}"))
    })?;
    match value.get("type").and_then(|value| value.as_str()) {
        Some("message_start") => {
            if let Some(mut usage) = value
                .get("message")
                .and_then(|message| message.get("usage"))
                .and_then(parse_usage)
            {
                // Anthropic's message_start may include a seed output token count,
                // while later message_delta usage reports output progress. The TUI
                // merges usage events by summing fields, so only emit input/cache
                // counts from the start event to avoid double-counting output.
                usage.output_tokens = None;
                on_event(ModelEvent::Usage(usage_to_model_usage(usage)))?;
            }
        }
        Some("content_block_start") => {
            let index = content_index(&value)?;
            let block = state.ensure_block(index);
            if value
                .get("content_block")
                .and_then(|content_block| content_block.get("type"))
                .and_then(|kind| kind.as_str())
                == Some("tool_use")
            {
                let content_block = &value["content_block"];
                if let Some(id) = content_block.get("id").and_then(|id| id.as_str()) {
                    block.tool_id = Some(id.to_string());
                }
                if let Some(name) = content_block.get("name").and_then(|name| name.as_str()) {
                    block.tool_name = Some(name.to_string());
                }
                if let Some(input) = content_block.get("input").filter(|input| !input.is_null()) {
                    let initial = serde_json::to_string(input).map_err(|err| {
                        ModelError::InvalidResponse(format!(
                            "invalid streamed tool_use input JSON: {err}"
                        ))
                    })?;
                    if initial != "{}" {
                        block.tool_input.push_str(&initial);
                    }
                }
            }
        }
        Some("content_block_delta") => {
            let index = content_index(&value)?;
            let block = state.ensure_block(index);
            let delta = &value["delta"];
            match delta.get("type").and_then(|kind| kind.as_str()) {
                Some("text_delta") => {
                    if let Some(text) = delta.get("text").and_then(|text| text.as_str()) {
                        block.text.push_str(text);
                        on_event(ModelEvent::OutputDelta(text.to_string()))?;
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial_json) = delta
                        .get("partial_json")
                        .and_then(|partial_json| partial_json.as_str())
                    {
                        block.tool_input.push_str(partial_json);
                    }
                }
                Some(_) | None => {}
            }
        }
        Some("message_delta") => {
            if let Some(mut usage) = value.get("usage").and_then(parse_usage) {
                let cumulative = usage.output_tokens.unwrap_or(0);
                let delta = cumulative.saturating_sub(state.last_output_tokens);
                state.last_output_tokens = cumulative;
                usage.output_tokens = Some(delta);
                on_event(ModelEvent::Usage(usage_to_model_usage(usage)))?;
            }
        }
        Some("error") => {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .unwrap_or("Anthropic stream returned an error");
            return Err(ModelError::InvalidResponse(message.to_string()));
        }
        Some("content_block_stop") | Some("message_stop") | Some("ping") | None => {}
        Some(_) => {}
    }
    Ok(())
}

fn content_index(value: &serde_json::Value) -> Result<usize, ModelError> {
    value
        .get("index")
        .and_then(|index| index.as_u64())
        .map(|index| index as usize)
        .ok_or_else(|| ModelError::InvalidResponse("Anthropic stream event missing index".into()))
}

fn parse_usage(value: &serde_json::Value) -> Option<AnthropicUsage> {
    serde_json::from_value(value.clone()).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ContentBlock;

    #[test]
    fn streams_text_deltas_and_usage() {
        let mut state = AnthropicSseState::default();
        let mut events = Vec::new();
        let mut on_event = |event| {
            events.push(event);
            Ok(())
        };

        handle_anthropic_stream_line(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1}}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();
        handle_anthropic_stream_line(
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();
        handle_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"he"}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();
        handle_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"llo"}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();

        assert!(events.iter().any(|event| {
            matches!(
                event,
                ModelEvent::Usage(usage)
                    if usage.input_tokens == Some(7) && usage.output_tokens.is_none()
            )
        }));
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    ModelEvent::OutputDelta(delta) => Some(delta.as_str()),
                    ModelEvent::ReasoningDelta(_)
                    | ModelEvent::WebSearch(_)
                    | ModelEvent::Usage(_) => None,
                })
                .collect::<String>(),
            "hello"
        );
        let ModelResponse::Assistant(blocks) = state.into_response().unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text(text) if text == "hello"));
    }

    #[test]
    fn streams_message_delta_usage_as_output_only() {
        let mut state = AnthropicSseState::default();
        let mut events = Vec::new();
        let mut on_event = |event| {
            events.push(event);
            Ok(())
        };

        handle_anthropic_stream_line(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1}}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();
        handle_anthropic_stream_line(
            r#"data: {"type":"message_delta","usage":{"output_tokens":5}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();

        let usages = events
            .iter()
            .filter_map(|event| match event {
                ModelEvent::Usage(usage) => Some(usage),
                ModelEvent::OutputDelta(_)
                | ModelEvent::ReasoningDelta(_)
                | ModelEvent::WebSearch(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(usages.len(), 2);
        assert_eq!(usages[0].input_tokens, Some(7));
        assert_eq!(usages[0].output_tokens, None);
        assert_eq!(usages[1].input_tokens, None);
        assert_eq!(usages[1].output_tokens, Some(5));
    }

    #[test]
    fn streams_tool_use_input_json_deltas() {
        let mut state = AnthropicSseState::default();
        let mut on_event = |_event| Ok(());

        handle_anthropic_stream_line(
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();
        handle_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();
        handle_anthropic_stream_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"pwd\"}"}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap();

        let ModelResponse::Assistant(blocks) = state.into_response().unwrap();
        assert!(matches!(
            &blocks[0],
            ContentBlock::ToolCall(call)
                if call.id == "toolu_1" && call.name == "bash" && call.arguments == json!({"command":"pwd"})
        ));
    }

    #[test]
    fn stream_error_event_returns_error() {
        let mut state = AnthropicSseState::default();
        let mut on_event = |_event| Ok(());
        let err = handle_anthropic_stream_line(
            r#"data: {"type":"error","error":{"message":"bad request"}}"#,
            &mut state,
            &mut on_event,
        )
        .unwrap_err();

        assert!(err.to_string().contains("bad request"));
    }
}
