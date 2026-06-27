use futures_util::StreamExt;
use serde_json::{json, Value};

pub(crate) mod auth;
pub mod cache;
mod codex_ws;
pub(crate) mod convert;
pub(crate) mod stream;
pub(crate) mod types;

pub use cache::prompt_cache_key_from_session_id;

use auth::{
    load_api_key_auth, load_codex_auth, load_codex_tokens_for_request, refresh_codex_token, Auth,
    CodexAuthSource,
};
use codex_ws::{CodexWsTransport, CodexWsTurn};
use convert::{
    codex_input_items, codex_reasoning_param, convert_openai_response, to_openai_message,
    to_openai_tool, to_responses_tool,
};
use stream::{
    collect_codex_sse_response, convert_streamed_response, handle_openai_stream_line,
    trim_sse_line_end,
};
use types::{ChatRequest, ChatResponse, ChatStreamOptions};

use crate::{
    credentials::CodexTokens,
    model::{AuthMode, ModelError, ModelEvent, ModelProvider, ModelRequest, ModelResponse},
};

pub struct OpenAiProvider {
    client: reqwest::Client,
    auth: Auth,
    api_base: String,
    model: String,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
    codex_ws: CodexWsTransport,
}

impl OpenAiProvider {
    pub fn new_with_reasoning(
        model: String,
        mode: AuthMode,
        reasoning_effort: Option<String>,
        reasoning_summary: Option<String>,
    ) -> Result<Self, ModelError> {
        let auth = match mode {
            AuthMode::ApiKey => load_api_key_auth()?,
            AuthMode::Codex => load_codex_auth()?,
        };
        let api_base: String = match &auth {
            Auth::Codex { .. } => "https://chatgpt.com/backend-api/codex".into(),
            Auth::ApiKey(_) => "https://api.openai.com/v1".into(),
        };
        let codex_ws = CodexWsTransport::new(&api_base);
        Ok(Self {
            client: reqwest::Client::new(),
            auth,
            api_base,
            model,
            reasoning_effort,
            reasoning_summary,
            codex_ws,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for OpenAiProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        match &self.auth {
            Auth::ApiKey(key) => self.send_chat_completions(request, key).await,
            Auth::Codex { tokens, source } => {
                let request_tokens = load_codex_tokens_for_request(tokens, *source)?;
                self.send_codex_responses(request, request_tokens, *source)
                    .await
            }
        }
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        match &self.auth {
            Auth::ApiKey(key) => {
                self.send_chat_completions_stream(request.clone(), key, on_event)
                    .await
            }
            Auth::Codex { tokens, source } => {
                let request_tokens = load_codex_tokens_for_request(tokens, *source)?;
                self.send_codex_responses_stream(request, request_tokens, *source, on_event)
                    .await
            }
        }
    }
}

impl OpenAiProvider {
    async fn send_chat_completions(
        &self,
        request: ModelRequest,
        key: &str,
    ) -> Result<ModelResponse, ModelError> {
        let messages = request
            .messages
            .into_iter()
            .map(to_openai_message)
            .collect::<Result<Vec<_>, _>>()?;
        let tools = request
            .tools
            .into_iter()
            .map(to_openai_tool)
            .collect::<Vec<_>>();
        let has_tools = !tools.is_empty();
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response: ChatResponse = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
                tools: has_tools.then_some(tools),
                tool_choice: has_tools.then_some("auto"),
                stream: false,
                stream_options: None,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        convert_openai_response(response)
    }

    async fn send_chat_completions_stream(
        &self,
        request: ModelRequest,
        key: &str,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let messages = request
            .messages
            .into_iter()
            .map(to_openai_message)
            .collect::<Result<Vec<_>, _>>()?;
        let tools = request
            .tools
            .into_iter()
            .map(to_openai_tool)
            .collect::<Vec<_>>();
        let has_tools = !tools.is_empty();
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
                tools: has_tools.then_some(tools),
                tool_choice: has_tools.then_some("auto"),
                stream: true,
                stream_options: Some(ChatStreamOptions {
                    include_usage: true,
                }),
            })
            .send()
            .await?
            .error_for_status()?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();
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
                handle_openai_stream_line(line, &mut text, &mut tool_calls, on_event)?;
            }
        }
        if !buffer.is_empty() {
            trim_sse_line_end(&mut buffer);
            let line = std::str::from_utf8(&buffer).map_err(|err| {
                ModelError::InvalidResponse(format!(
                    "streamed response contained invalid utf-8: {err}"
                ))
            })?;
            handle_openai_stream_line(line, &mut text, &mut tool_calls, on_event)?;
        }

        convert_streamed_response(text, tool_calls)
    }

    async fn send_codex_responses(
        &self,
        request: ModelRequest,
        tokens: CodexTokens,
        source: CodexAuthSource,
    ) -> Result<ModelResponse, ModelError> {
        self.send_codex_responses_inner(request, tokens, source, None)
            .await
    }

    async fn send_codex_responses_stream(
        &self,
        request: ModelRequest,
        tokens: CodexTokens,
        source: CodexAuthSource,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        self.send_codex_responses_inner(request, tokens, source, Some(on_event))
            .await
    }

    async fn send_codex_responses_inner(
        &self,
        request: ModelRequest,
        tokens: CodexTokens,
        source: CodexAuthSource,
        mut on_event: Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
    ) -> Result<ModelResponse, ModelError> {
        let body = build_codex_responses_body(
            &self.model,
            request,
            self.reasoning_effort.as_deref(),
            self.reasoning_summary.as_deref(),
        )?;
        match self
            .codex_ws
            .send_responses_turn(body.clone(), &tokens, &mut on_event)
            .await?
        {
            CodexWsTurn::Completed(response) => return Ok(response),
            CodexWsTurn::FullSseFallback => {
                // The WebSocket transport has already reset stale continuation
                // state and withheld stream events, so replaying the full body
                // over SSE cannot duplicate caller-visible deltas.
            }
        }

        let url = format!("{}/responses", self.api_base.trim_end_matches('/'));
        let make_request = |token: &str| {
            self.client
                .post(&url)
                .bearer_auth(token)
                .header("User-Agent", "codex-cli")
                .header("originator", "codex_cli_rs")
                .json(&body)
        };
        let mut req = make_request(&tokens.access_token);
        if let Some(account_id) = tokens.account_id.as_deref() {
            req = req.header("ChatGPT-Account-ID", account_id);
        }
        let response = match req.send().await {
            Ok(response) => response,
            Err(err) => {
                self.codex_ws.reset().await;
                return Err(err.into());
            }
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.codex_ws.reset().await;
            if let Some(refresh_token) = tokens.refresh_token.as_deref() {
                let refreshed =
                    refresh_codex_token(&self.client, refresh_token, source, &tokens).await?;
                let mut req = make_request(&refreshed.access_token);
                if let Some(account_id) = refreshed.account_id.as_deref() {
                    req = req.header("ChatGPT-Account-ID", account_id);
                }
                let response = match req.send().await {
                    Ok(response) => response,
                    Err(err) => {
                        self.codex_ws.reset().await;
                        return Err(err.into());
                    }
                };
                if !response.status().is_success() {
                    self.codex_ws.reset().await;
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ModelError::HttpStatus { status, body });
                }
                return self
                    .collect_codex_sse_response(response, &mut on_event, &body)
                    .await;
            }
        }
        if !response.status().is_success() {
            self.codex_ws.reset().await;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
        }
        self.collect_codex_sse_response(response, &mut on_event, &body)
            .await
    }

    async fn collect_codex_sse_response(
        &self,
        response: reqwest::Response,
        on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
        body: &Value,
    ) -> Result<ModelResponse, ModelError> {
        match collect_codex_sse_response(response, on_event).await {
            Ok(output) => {
                self.codex_ws
                    .record_full_request_success(body, output.response_id)
                    .await?;
                Ok(output.response)
            }
            Err(err) => {
                self.codex_ws.reset().await;
                Err(err)
            }
        }
    }
}

fn build_codex_responses_body(
    model: &str,
    request: ModelRequest,
    reasoning_effort: Option<&str>,
    reasoning_summary: Option<&str>,
) -> Result<Value, ModelError> {
    let mut instructions = Vec::new();
    let input = codex_input_items(request.messages, &mut instructions)?;
    let tools: Vec<_> = request.tools.into_iter().map(to_responses_tool).collect();
    let instructions = instructions.join("\n\n");
    let mut body = json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "store": false,
        "stream": true
    });

    if let Some(prompt_cache_key) = request.prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    if let Some(reasoning) = codex_reasoning_param(reasoning_effort, reasoning_summary) {
        body["reasoning"] = reasoning;
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::convert::{codex_reasoning_param, to_openai_message};
    use super::stream::{
        convert_streamed_response, extract_sse_text, handle_codex_sse_line,
        handle_openai_stream_line, trim_sse_line_end, CodexSseState,
    };
    use super::*;
    use crate::model::{ContentBlock, Message};
    use crate::tool::{ToolCall, ToolResult, ToolSpec};
    use serde_json::json;

    #[test]
    fn codex_reasoning_param_omits_none_values() {
        assert!(codex_reasoning_param(Some("none"), Some("none")).is_none());
        assert_eq!(
            codex_reasoning_param(Some("low"), Some("auto")).unwrap(),
            json!({"effort":"low","summary":"auto"})
        );
    }

    #[test]
    fn reasoning_level_maps_to_codex_reasoning_param() {
        assert!(codex_reasoning_param(
            crate::reasoning::ReasoningLevel::Off.effort(),
            crate::reasoning::ReasoningLevel::Off.summary()
        )
        .is_none());
        assert_eq!(
            codex_reasoning_param(
                crate::reasoning::ReasoningLevel::Minimal.effort(),
                crate::reasoning::ReasoningLevel::Minimal.summary()
            )
            .unwrap(),
            json!({"effort":"low","summary":"auto"})
        );
        assert_eq!(
            codex_reasoning_param(
                crate::reasoning::ReasoningLevel::Xhigh.effort(),
                crate::reasoning::ReasoningLevel::Xhigh.summary()
            )
            .unwrap(),
            json!({"effort":"xhigh","summary":"auto"})
        );
    }

    #[test]
    fn codex_responses_body_includes_prompt_cache_key_when_present() {
        let body = build_codex_responses_body(
            "gpt-5-codex",
            ModelRequest {
                messages: vec![Message::user_text("hello")],
                tools: Vec::new(),
                prompt_cache_key: Some("rho:session-1".into()),
            },
            None,
            None,
        )
        .unwrap();

        assert_eq!(body["prompt_cache_key"], "rho:session-1");
        assert!(body.get("previous_response_id").is_none());
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn codex_responses_body_omits_prompt_cache_key_when_absent() {
        let body = build_codex_responses_body(
            "gpt-5-codex",
            ModelRequest {
                messages: vec![Message::user_text("hello")],
                tools: Vec::new(),
                prompt_cache_key: None,
            },
            None,
            None,
        )
        .unwrap();

        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn codex_responses_body_uses_hosted_web_search_tool() {
        let body = build_codex_responses_body(
            "gpt-5-codex",
            ModelRequest {
                messages: vec![Message::user_text("find current docs")],
                tools: vec![ToolSpec {
                    name: "web_search".into(),
                    description: "search the web".into(),
                    input_schema: json!({"type": "object"}),
                }],
                prompt_cache_key: None,
            },
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            body["tools"],
            json!([{"type": "web_search", "external_web_access": true}])
        );
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn chat_completions_request_does_not_serialize_prompt_cache_key() {
        let body = serde_json::to_value(ChatRequest {
            model: "gpt-4.1".into(),
            messages: vec![super::types::OpenAiMessage {
                role: "user".into(),
                content: Some("hello".into()),
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: None,
            tool_choice: None,
            stream: false,
            stream_options: None,
        })
        .unwrap();

        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn extracts_sse_delta_text() {
        let body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\n",
            "data: [DONE]\n"
        );
        assert_eq!(extract_sse_text(body).unwrap(), "Hello world");
    }

    #[test]
    fn chat_stream_usage_normalizes_prompt_cached_tokens() {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut usage = None;
        handle_openai_stream_line(
            r#"data: {"usage":{"prompt_tokens":1000,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":700}},"choices":[{"delta":{}}]}"#,
            &mut text,
            &mut tool_calls,
            &mut |event| {
                match event {
                    ModelEvent::Usage(event_usage) => usage = Some(event_usage),
                    ModelEvent::OutputDelta(_)
                    | ModelEvent::ReasoningDelta(_)
                    | ModelEvent::WebSearch(_) => {}
                }
                Ok(())
            },
        )
        .unwrap();

        let usage = usage.unwrap();
        assert_eq!(usage.input_tokens, Some(300));
        assert_eq!(usage.cache_read_tokens, Some(700));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_input_tokens(), Some(1000));
    }

    #[test]
    fn codex_response_usage_normalizes_input_cached_tokens() {
        let mut state = CodexSseState::default();
        let mut usage = None;
        handle_codex_sse_line(
            r#"data: {"type":"response.completed","response":{"usage":{"input_tokens":1000,"output_tokens":25,"input_tokens_details":{"cached_tokens":700}},"output_text":"done","output":[]}}"#,
            &mut state,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::Usage(event_usage) => usage = Some(event_usage),
                    ModelEvent::OutputDelta(_)
                    | ModelEvent::ReasoningDelta(_)
                    | ModelEvent::WebSearch(_) => {}
                }
                Ok(())
            }),
        )
        .unwrap();

        let usage = usage.unwrap();
        assert_eq!(usage.input_tokens, Some(300));
        assert_eq!(usage.cache_read_tokens, Some(700));
        assert_eq!(usage.output_tokens, Some(25));
        assert_eq!(usage.total_input_tokens(), Some(1000));
    }

    #[test]
    fn codex_sse_line_emits_output_delta() {
        let mut state = CodexSseState::default();
        let mut deltas = Vec::new();
        handle_codex_sse_line(
            r#"data: {"type":"response.output_text.delta","delta":"hi"}"#,
            &mut state,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::OutputDelta(delta) => deltas.push(delta),
                    ModelEvent::ReasoningDelta(_) => {}
                    ModelEvent::WebSearch(_) => {}
                    ModelEvent::Usage(_) => {}
                }
                Ok(())
            }),
        )
        .unwrap();

        assert_eq!(state.text, "hi");
        assert_eq!(deltas, vec!["hi"]);
        assert!(state.completed_text.is_none());
    }

    #[test]
    fn codex_sse_line_emits_reasoning_summary_delta() {
        let mut state = CodexSseState::default();
        let mut deltas = Vec::new();
        handle_codex_sse_line(
            r#"data:{"type":"response.reasoning_summary_text.delta","delta":"thinking","summary_index":0}"#,
            &mut state,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::OutputDelta(_) => {}
                    ModelEvent::ReasoningDelta(delta) => deltas.push(delta),
                    ModelEvent::WebSearch(_) => {}
                    ModelEvent::Usage(_) => {},
                }
                Ok(())
            }),
        )
        .unwrap();

        assert!(state.text.is_empty());
        assert_eq!(deltas, vec!["thinking"]);
    }

    #[test]
    fn codex_sse_line_emits_reasoning_text_delta() {
        let mut state = CodexSseState::default();
        let mut deltas = Vec::new();
        handle_codex_sse_line(
            r#"data: {"type":"response.reasoning_text.delta","delta":"raw","content_index":0}"#,
            &mut state,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::OutputDelta(_) => {}
                    ModelEvent::ReasoningDelta(delta) => deltas.push(delta),
                    ModelEvent::WebSearch(_) => {}
                    ModelEvent::Usage(_) => {}
                }
                Ok(())
            }),
        )
        .unwrap();

        assert!(state.text.is_empty());
        assert_eq!(deltas, vec!["raw"]);
    }

    #[test]
    fn extracts_completed_response_text_when_no_deltas() {
        let body = r#"data: {"type":"response.completed","response":{"output_text":"done","output":null}}
"#;
        assert_eq!(extract_sse_text(body).unwrap(), "done");
    }

    #[test]
    fn completed_response_text_preserves_url_annotations() {
        let body = r#"data: {"type":"response.completed","response":{"output_text":"Rust shipped today.","output":[{"content":[{"text":"Rust shipped today.","annotations":[{"type":"url_citation","title":"Rust Blog","url":"https://blog.rust-lang.org/release"}]}]}]}}
"#;
        let text = extract_sse_text(body).unwrap();

        assert!(text.contains("Rust shipped today."));
        assert!(text.contains("Sources:"));
        assert!(text.contains("Rust Blog: https://blog.rust-lang.org/release"));
    }

    #[test]
    fn codex_sse_line_collects_response_id() {
        let mut state = CodexSseState::default();
        handle_codex_sse_line(
            r#"data: {"type":"response.completed","response":{"id":"resp_123","output_text":"done","output":null}}"#,
            &mut state,
            &mut None,
        )
        .unwrap();

        let response = state.into_response().unwrap();
        assert_eq!(response.response_id.as_deref(), Some("resp_123"));
    }

    #[test]
    fn codex_sse_line_collects_function_call() {
        let mut state = CodexSseState::default();
        handle_codex_sse_line(
            r#"data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call-1","name":"bash","arguments":"{\"command\":\"pwd\"}"}}"#,
            &mut state,
            &mut None,
        )
        .unwrap();

        let response = state.into_response().unwrap();
        let ModelResponse::Assistant(blocks) = response.response;
        assert!(matches!(
            blocks.as_slice(),
            [ContentBlock::ToolCall(ToolCall { id, name, arguments })]
                if id == "call-1" && name == "bash" && arguments == &json!({ "command": "pwd" })
        ));
    }

    #[test]
    fn codex_sse_line_emits_web_search_detail() {
        let mut state = CodexSseState::default();
        let mut searches = Vec::new();
        handle_codex_sse_line(
            r#"data: {"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","action":{"type":"search","query":"latest Rust release"}}}"#,
            &mut state,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::WebSearch(detail) => searches.push(detail),
                    ModelEvent::OutputDelta(_) => {}
                    ModelEvent::ReasoningDelta(_) => {}
                    ModelEvent::Usage(_) => {}
                }
                Ok(())
            }),
        )
        .unwrap();

        assert_eq!(searches, vec!["for \"latest Rust release\""]);
    }

    #[test]
    fn parses_chat_completion_stream_line_as_output_delta() {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut deltas = Vec::new();
        handle_openai_stream_line(
            r#"data: {"choices":[{"delta":{"content":"hé"}}]}"#,
            &mut text,
            &mut tool_calls,
            &mut |event| {
                match event {
                    ModelEvent::OutputDelta(delta) => deltas.push(delta),
                    ModelEvent::ReasoningDelta(_) => {}
                    ModelEvent::WebSearch(_) => {}
                    ModelEvent::Usage(_) => {}
                }
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(text, "hé");
        assert_eq!(deltas, vec!["hé"]);
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn accumulates_streamed_tool_call_deltas() {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        handle_openai_stream_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"bash","arguments":"{\"command\":"}}]}}]}"#,
            &mut text,
            &mut tool_calls,
            &mut |_| Ok(()),
        )
        .unwrap();
        handle_openai_stream_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"pwd\"}"}}]}}]}"#,
            &mut text,
            &mut tool_calls,
            &mut |_| Ok(()),
        )
        .unwrap();

        let response = convert_streamed_response(text, tool_calls).unwrap();
        let ModelResponse::Assistant(blocks) = response;
        assert!(matches!(
            blocks.as_slice(),
            [ContentBlock::ToolCall(ToolCall { id, name, arguments })]
                if id == "call-1" && name == "bash" && arguments == &json!({ "command": "pwd" })
        ));
    }

    #[test]
    fn parses_chat_completion_stream_line_as_reasoning_delta() {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut deltas = Vec::new();
        handle_openai_stream_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"thinking"}}]}"#,
            &mut text,
            &mut tool_calls,
            &mut |event| {
                match event {
                    ModelEvent::OutputDelta(_) => {}
                    ModelEvent::ReasoningDelta(delta) => deltas.push(delta),
                    ModelEvent::WebSearch(_) => {}
                    ModelEvent::Usage(_) => {}
                }
                Ok(())
            },
        )
        .unwrap();

        assert!(text.is_empty());
        assert_eq!(deltas, vec!["thinking"]);
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn trims_sse_line_end_after_full_utf8_line() {
        let mut line = "data: hé\r\n".as_bytes().to_vec();
        trim_sse_line_end(&mut line);

        assert_eq!(std::str::from_utf8(&line).unwrap(), "data: hé");
    }

    #[test]
    fn serializes_openai_native_tool_result() {
        let message = to_openai_message(Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "done".into(),
        }))
        .unwrap();
        assert_eq!(message.role, "tool");
        assert_eq!(message.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(message.content.as_deref(), Some("done"));
    }
}
