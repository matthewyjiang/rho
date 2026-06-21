use futures_util::StreamExt;
use serde_json::json;

mod auth;
mod convert;
mod stream;
mod types;

use auth::{load_codex_auth, load_codex_tokens_from_path, refresh_codex_token, Auth};
use convert::{
    codex_input_items, codex_reasoning_param, convert_openai_response, to_openai_message,
    to_openai_tool, to_responses_tool,
};
use stream::{
    collect_codex_sse_response, convert_streamed_response, handle_openai_stream_line,
    trim_sse_line_end,
};
use types::{ChatRequest, ChatResponse};

use crate::model::{AuthMode, ModelError, ModelEvent, ModelProvider, ModelRequest, ModelResponse};

pub struct OpenAiProvider {
    client: reqwest::Client,
    auth: Auth,
    api_base: String,
    model: String,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
}

impl OpenAiProvider {
    pub fn new_with_reasoning(
        model: String,
        mode: AuthMode,
        reasoning_effort: Option<String>,
        reasoning_summary: Option<String>,
    ) -> Result<Self, ModelError> {
        let auth = match mode {
            AuthMode::ApiKey => Auth::ApiKey(
                std::env::var("OPENAI_API_KEY").map_err(|_| ModelError::MissingApiKey)?,
            ),
            AuthMode::Codex => load_codex_auth()?,
        };
        let api_base = match &auth {
            Auth::Codex { .. } => "https://chatgpt.com/backend-api/codex".into(),
            Auth::ApiKey(_) => "https://api.openai.com/v1".into(),
        };
        Ok(Self {
            client: reqwest::Client::new(),
            auth,
            api_base,
            model,
            reasoning_effort,
            reasoning_summary,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for OpenAiProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        match &self.auth {
            Auth::ApiKey(key) => self.send_chat_completions(request, key).await,
            Auth::Codex {
                access_token,
                refresh_token,
                account_id,
                auth_path,
            } => {
                let auth_path_for_request = auth_path.clone();
                let (access_token, refresh_token, account_id) = if let Some(path) = auth_path {
                    let tokens = load_codex_tokens_from_path(path)?;
                    (
                        tokens.access_token,
                        tokens.refresh_token,
                        tokens.account_id.or_else(|| account_id.clone()),
                    )
                } else {
                    (
                        access_token.clone(),
                        refresh_token.clone(),
                        account_id.clone(),
                    )
                };
                self.send_codex_responses(
                    request,
                    &access_token,
                    refresh_token.as_deref(),
                    account_id.as_deref(),
                    auth_path_for_request.as_deref(),
                )
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
            Auth::Codex {
                access_token,
                refresh_token,
                account_id,
                auth_path,
            } => {
                let (access_token, refresh_token, account_id, auth_path_for_request) = {
                    (
                        access_token.clone(),
                        refresh_token.clone(),
                        account_id.clone(),
                        auth_path.clone(),
                    )
                };
                self.send_codex_responses_stream(
                    request,
                    &access_token,
                    refresh_token.as_deref(),
                    account_id.as_deref(),
                    auth_path_for_request.as_deref(),
                    on_event,
                )
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
        let tools = request.tools.into_iter().map(to_openai_tool).collect();
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response: ChatResponse = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
                tools,
                tool_choice: "auto",
                stream: false,
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
        let tools = request.tools.into_iter().map(to_openai_tool).collect();
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
                tools,
                tool_choice: "auto",
                stream: true,
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
        token: &str,
        refresh_token: Option<&str>,
        account_id: Option<&str>,
        auth_path: Option<&std::path::Path>,
    ) -> Result<ModelResponse, ModelError> {
        self.send_codex_responses_inner(request, token, refresh_token, account_id, auth_path, None)
            .await
    }

    async fn send_codex_responses_stream(
        &self,
        request: ModelRequest,
        token: &str,
        refresh_token: Option<&str>,
        account_id: Option<&str>,
        auth_path: Option<&std::path::Path>,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        self.send_codex_responses_inner(
            request,
            token,
            refresh_token,
            account_id,
            auth_path,
            Some(on_event),
        )
        .await
    }

    async fn send_codex_responses_inner(
        &self,
        request: ModelRequest,
        token: &str,
        refresh_token: Option<&str>,
        account_id: Option<&str>,
        auth_path: Option<&std::path::Path>,
        mut on_event: Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
    ) -> Result<ModelResponse, ModelError> {
        let mut instructions = Vec::new();
        let input = codex_input_items(request.messages, &mut instructions)?;
        let tools: Vec<_> = request.tools.into_iter().map(to_responses_tool).collect();
        let instructions = instructions.join("\n\n");
        let url = format!("{}/responses", self.api_base.trim_end_matches('/'));
        let make_body = || {
            let mut body = json!({
                "model": self.model,
                "instructions": instructions,
                "input": input,
                "tools": tools,
                "tool_choice": "auto",
                "store": false,
                "stream": true
            });
            if let Some(reasoning) = codex_reasoning_param(
                self.reasoning_effort.as_deref(),
                self.reasoning_summary.as_deref(),
            ) {
                body["reasoning"] = reasoning;
            }
            body
        };
        let make_request = |token: &str| {
            self.client
                .post(&url)
                .bearer_auth(token)
                .header("User-Agent", "codex-cli")
                .header("originator", "codex_cli_rs")
                .json(&make_body())
        };
        let mut req = make_request(token);
        if let Some(account_id) = account_id {
            req = req.header("ChatGPT-Account-ID", account_id);
        }
        let response = req.send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(refresh_token) = refresh_token {
                let refreshed = refresh_codex_token(&self.client, refresh_token, auth_path).await?;
                let mut req = make_request(&refreshed.access_token);
                if let Some(account_id) = refreshed.account_id.as_deref().or(account_id) {
                    req = req.header("ChatGPT-Account-ID", account_id);
                }
                let response = req.send().await?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ModelError::HttpStatus { status, body });
                }
                return collect_codex_sse_response(response, &mut on_event).await;
            }
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
        }
        collect_codex_sse_response(response, &mut on_event).await
    }
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
    use crate::tool::{ToolCall, ToolResult};
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
    fn codex_sse_line_collects_function_call() {
        let mut state = CodexSseState::default();
        handle_codex_sse_line(
            r#"data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call-1","name":"bash","arguments":"{\"command\":\"pwd\"}"}}"#,
            &mut state,
            &mut None,
        )
        .unwrap();

        let response = state.into_response().unwrap();
        let ModelResponse::Assistant(blocks) = response;
        assert!(matches!(
            blocks.as_slice(),
            [ContentBlock::ToolCall(ToolCall { id, name, arguments })]
                if id == "call-1" && name == "bash" && arguments == &json!({ "command": "pwd" })
        ));
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
