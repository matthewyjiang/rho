use futures_util::StreamExt;

use crate::{
    model::{ModelUsage, ProviderReportedErrorKind},
    protocol::cost::parse_usd_micros,
    provider_backend::{ModelError, ModelEvent, ModelResponse},
};

use super::convert::{convert_content_blocks, usage_to_model_usage};
use super::types::{AnthropicContentBlock, AnthropicUsage};
use crate::provider_backend::line_decoder::LineDecoder;

const MAX_STREAM_BLOCK_INDEX: usize = 4096;

#[derive(Default)]
pub(crate) struct AnthropicSseState {
    blocks: Vec<StreamedBlock>,
    last_output_tokens: u64,
    last_reported_cost_usd_micros: u64,
}

#[derive(Default)]
struct StreamedBlock {
    text: String,
    tool_id: Option<String>,
    tool_name: Option<String>,
    tool_input: String,
    thinking: String,
    signature: String,
    redacted_thinking: Option<String>,
}

impl AnthropicSseState {
    fn ensure_block(&mut self, index: usize) -> &mut StreamedBlock {
        while self.blocks.len() <= index {
            self.blocks.push(StreamedBlock::default());
        }
        &mut self.blocks[index]
    }

    pub(crate) fn into_response(self) -> Result<ModelResponse, ModelError> {
        let mut blocks = Vec::new();
        for (index, block) in self.blocks.into_iter().enumerate() {
            if !block.text.is_empty() {
                blocks.push(AnthropicContentBlock::Text {
                    text: block.text,
                    cache_control: None,
                });
            }
            if !block.thinking.is_empty() || !block.signature.is_empty() {
                blocks.push(AnthropicContentBlock::Thinking {
                    thinking: block.thinking,
                    signature: block.signature,
                });
            }
            if let Some(data) = block.redacted_thinking {
                blocks.push(AnthropicContentBlock::RedactedThinking { data });
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

pub(crate) async fn collect_anthropic_sse_response(
    response: reqwest::Response,
    on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
) -> Result<ModelResponse, ModelError> {
    let mut state = AnthropicSseState::default();
    let mut decoder = LineDecoder::default();
    let mut stream = response.bytes_stream();
    let mut idle_deadline = crate::provider_backend::stream_timeout::StreamIdleDeadline::new();
    loop {
        let Some(chunk) = idle_deadline.wait_for(stream.next()).await? else {
            break;
        };
        decoder.push(&chunk?);
        while let Some(line) = decoder.next_line().map_err(invalid_stream_utf8)? {
            if handle_anthropic_stream_line(line, &mut state, on_event)? {
                idle_deadline.record_activity();
            }
        }
    }
    if let Some(line) = decoder.finish().map_err(invalid_stream_utf8)? {
        handle_anthropic_stream_line(line, &mut state, on_event)?;
    }
    state.into_response()
}

fn invalid_stream_utf8(err: std::str::Utf8Error) -> ModelError {
    ModelError::InvalidResponse(format!("streamed response contained invalid utf-8: {err}"))
}

fn sse_data(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("data:")?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

pub(crate) fn handle_anthropic_stream_line(
    line: &str,
    state: &mut AnthropicSseState,
    on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
) -> Result<bool, ModelError> {
    let Some(data) = sse_data(line) else {
        return Ok(false);
    };
    if data == "[DONE]" {
        return Ok(true);
    }
    let value = serde_json::from_str::<serde_json::Value>(data).map_err(|err| {
        ModelError::InvalidResponse(format!("invalid Anthropic stream JSON: {err}"))
    })?;
    if value.get("type").and_then(|value| value.as_str()) == Some("ping") {
        return Ok(false);
    }
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
            let content_block = &value["content_block"];
            match content_block.get("type").and_then(|kind| kind.as_str()) {
                Some("thinking") => {
                    block.thinking = content_block
                        .get("thinking")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    block.signature = content_block
                        .get("signature")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                }
                Some("redacted_thinking") => {
                    block.redacted_thinking = content_block
                        .get("data")
                        .and_then(|value| value.as_str())
                        .map(str::to_string);
                }
                Some("tool_use") => {
                    if let Some(id) = content_block.get("id").and_then(|id| id.as_str()) {
                        block.tool_id = Some(id.to_string());
                    }
                    if let Some(name) = content_block.get("name").and_then(|name| name.as_str()) {
                        block.tool_name = Some(name.to_string());
                    }
                    if let Some(input) = content_block.get("input").filter(|input| !input.is_null())
                    {
                        let initial = serde_json::to_string(input).map_err(|err| {
                            ModelError::InvalidResponse(format!(
                                "invalid streamed tool_use input JSON: {err}"
                            ))
                        })?;
                        if initial != "{}" {
                            block.tool_input.push_str(&initial);
                        }
                    }
                    on_event(ModelEvent::ToolCallDelta {
                        index,
                        id: block.tool_id.clone(),
                        name: block.tool_name.clone(),
                        arguments: block.tool_input.clone(),
                    })?;
                }
                Some(_) | None => {}
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
                        on_event(ModelEvent::ToolCallDelta {
                            index,
                            id: None,
                            name: None,
                            arguments: partial_json.to_string(),
                        })?;
                    }
                }
                Some("thinking_delta") => {
                    if let Some(thinking) = delta.get("thinking").and_then(|value| value.as_str()) {
                        block.thinking.push_str(thinking);
                        on_event(ModelEvent::ReasoningDelta(thinking.to_string()))?;
                    }
                }
                Some("signature_delta") => {
                    if let Some(signature) = delta.get("signature").and_then(|value| value.as_str())
                    {
                        block.signature.push_str(signature);
                    }
                }
                Some(_) | None => {}
            }
        }
        Some("message_delta") => {
            let output_tokens = value
                .get("usage")
                .and_then(parse_usage)
                .and_then(|usage| usage.output_tokens)
                .map(|cumulative| {
                    let delta = cumulative.saturating_sub(state.last_output_tokens);
                    state.last_output_tokens = state.last_output_tokens.max(cumulative);
                    delta
                });
            let reported_cost = value
                .get("provider_metadata")
                .and_then(|metadata| metadata.get("gateway"))
                .and_then(|gateway| gateway.get("cost"))
                .and_then(parse_usd_micros);
            if output_tokens.is_some() || reported_cost.is_some() {
                let cost_usd_micros = reported_cost.map(|cumulative| {
                    let delta = cumulative.saturating_sub(state.last_reported_cost_usd_micros);
                    state.last_reported_cost_usd_micros =
                        state.last_reported_cost_usd_micros.max(cumulative);
                    delta
                });
                on_event(ModelEvent::Usage(ModelUsage {
                    output_tokens,
                    total_tokens: output_tokens,
                    cost_usd_micros,
                    ..ModelUsage::default()
                }))?;
            }
        }
        Some("error") => {
            let error = value.get("error");
            let message = error
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .unwrap_or("Anthropic stream returned an error");
            let error_type = error
                .and_then(|error| error.get("type"))
                .and_then(|error_type| error_type.as_str());
            return Err(match error_type {
                Some(error_type) => ModelError::ProviderReported {
                    kind: anthropic_error_kind(error_type),
                    error_type: error_type.to_string(),
                    message: message.to_string(),
                },
                None => ModelError::InvalidResponse(message.to_string()),
            });
        }
        Some("content_block_stop") => {
            let index = content_index(&value)?;
            let block = state.ensure_block(index);
            let provider_block = if !block.thinking.is_empty() || !block.signature.is_empty() {
                Some(AnthropicContentBlock::Thinking {
                    thinking: block.thinking.clone(),
                    signature: block.signature.clone(),
                })
            } else {
                block
                    .redacted_thinking
                    .clone()
                    .map(|data| AnthropicContentBlock::RedactedThinking { data })
            };
            if let Some(provider_block) = provider_block {
                on_event(ModelEvent::ProviderContext {
                    kind: "anthropic_content_block".into(),
                    position: Some(index),
                    data: serde_json::to_value(provider_block).map_err(|err| {
                        ModelError::InvalidResponse(format!(
                            "could not retain Anthropic thinking block: {err}"
                        ))
                    })?,
                })?;
            }
        }
        Some("message_stop") | Some("ping") | None => {}
        Some(_) => {}
    }
    Ok(true)
}

fn anthropic_error_kind(error_type: &str) -> ProviderReportedErrorKind {
    match error_type {
        "timeout_error" => ProviderReportedErrorKind::Timeout,
        "rate_limit_error" => ProviderReportedErrorKind::RateLimit,
        "overloaded_error" | "api_error" => ProviderReportedErrorKind::Unavailable,
        _ => ProviderReportedErrorKind::InvalidResponse,
    }
}

fn content_index(value: &serde_json::Value) -> Result<usize, ModelError> {
    let index = value
        .get("index")
        .and_then(|index| index.as_u64())
        .ok_or_else(|| {
            ModelError::InvalidResponse("Anthropic stream event missing index".into())
        })?;
    let index = usize::try_from(index).map_err(|_| {
        ModelError::InvalidResponse(format!("stream block index {index} out of range"))
    })?;
    if index > MAX_STREAM_BLOCK_INDEX {
        return Err(ModelError::InvalidResponse(format!(
            "stream block index {index} out of range"
        )));
    }
    Ok(index)
}

fn parse_usage(value: &serde_json::Value) -> Option<AnthropicUsage> {
    serde_json::from_value(value.clone()).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::provider_backend::ContentBlock;

    #[test]
    fn stream_error_events_map_to_the_provider_contract() {
        use rho_sdk::ProviderErrorKind;

        let cases = [
            ("overloaded_error", ProviderErrorKind::Unavailable, true),
            ("api_error", ProviderErrorKind::Unavailable, true),
            ("rate_limit_error", ProviderErrorKind::RateLimit, true),
            ("timeout_error", ProviderErrorKind::Timeout, true),
            (
                "invalid_request_error",
                ProviderErrorKind::InvalidResponse,
                false,
            ),
        ];

        for (error_type, expected_kind, retryable) in cases {
            let mut state = AnthropicSseState::default();
            let line = format!(
                r#"data: {{"type":"error","error":{{"type":"{error_type}","message":"provider details"}}}}"#
            );
            let model_error =
                handle_anthropic_stream_line(&line, &mut state, &mut |_| Ok(())).unwrap_err();
            let error =
                crate::providers::sdk_contract::provider_error_from_model_error(model_error);
            let expected_diagnostic = format!("{error_type}: provider details");

            assert_eq!(error.kind(), expected_kind, "{error_type}");
            assert_eq!(error.is_retryable(), retryable, "{error_type}");
            assert_eq!(
                error.diagnostic(),
                Some(expected_diagnostic.as_str()),
                "{error_type}"
            );
        }
    }

    #[test]
    fn stream_error_events_without_a_type_stay_invalid_responses() {
        let mut state = AnthropicSseState::default();

        let error = handle_anthropic_stream_line(
            r#"data: {"type":"error","error":{"message":"broken"}}"#,
            &mut state,
            &mut |_| Ok(()),
        )
        .unwrap_err();

        assert!(matches!(error, ModelError::InvalidResponse(message) if message == "broken"));
    }

    #[test]
    fn ping_is_not_meaningful_stream_activity() {
        let mut state = AnthropicSseState::default();

        let activity =
            handle_anthropic_stream_line(r#"data: {"type":"ping"}"#, &mut state, &mut |_| Ok(()))
                .unwrap();

        assert!(!activity);
    }

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
                    | ModelEvent::ReasoningSummaryDelta(_)
                    | ModelEvent::ProviderContext { .. }
                    | ModelEvent::WebSearch(_)
                    | ModelEvent::Usage(_)
                    | ModelEvent::ToolCallDelta { .. } => None,
                })
                .collect::<String>(),
            "hello"
        );
        let ModelResponse::Assistant(blocks) = state.into_response().unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text(text) if text == "hello"));
    }

    #[test]
    fn streams_and_retains_signed_thinking_context() {
        let mut state = AnthropicSseState::default();
        let mut events = Vec::new();
        let mut on_event = |event| {
            events.push(event);
            Ok(())
        };

        for line in [
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"private"}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}"#,
        ] {
            handle_anthropic_stream_line(line, &mut state, &mut on_event).unwrap();
        }

        assert!(events.iter().any(|event| matches!(
            event,
            ModelEvent::ProviderContext { kind, position: Some(0), data }
                if kind == "anthropic_content_block"
                    && data["thinking"] == "private"
                    && data["signature"] == "signed"
        )));
        let ModelResponse::Assistant(blocks) = state.into_response().unwrap();
        assert!(matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "answer"));
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
                | ModelEvent::ReasoningSummaryDelta(_)
                | ModelEvent::ProviderContext { .. }
                | ModelEvent::WebSearch(_)
                | ModelEvent::ToolCallDelta { .. } => None,
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

#[cfg(test)]
#[path = "stream_cost_tests.rs"]
mod stream_cost_tests;

#[cfg(test)]
#[path = "stream_index_tests.rs"]
mod stream_index_tests;
