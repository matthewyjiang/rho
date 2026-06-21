use std::path::PathBuf;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::{
    AuthMode, ContentBlock, Message, ModelError, ModelEvent, ModelProvider, ModelRequest,
    ModelResponse,
};
use crate::tool::{ToolCall, ToolSpec};

pub struct OpenAiProvider {
    client: reqwest::Client,
    auth: Auth,
    api_base: String,
    model: String,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
}

enum Auth {
    ApiKey(String),
    Codex {
        access_token: String,
        refresh_token: Option<String>,
        account_id: Option<String>,
        auth_path: Option<PathBuf>,
    },
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
                let content = self
                    .send_codex_responses(
                        request.messages,
                        &access_token,
                        refresh_token.as_deref(),
                        account_id.as_deref(),
                        auth_path_for_request.as_deref(),
                    )
                    .await?;
                if let Some(call) = crate::prompt::parse_tool_call(&content)? {
                    Ok(ModelResponse::tool_call(call))
                } else {
                    Ok(ModelResponse::final_answer(content))
                }
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
                let content = self
                    .send_codex_responses_stream(
                        request.messages,
                        &access_token,
                        refresh_token.as_deref(),
                        account_id.as_deref(),
                        auth_path_for_request.as_deref(),
                        on_event,
                    )
                    .await?;
                if let Some(call) = crate::prompt::parse_tool_call(&content)? {
                    Ok(ModelResponse::tool_call(call))
                } else {
                    Ok(ModelResponse::final_answer(content))
                }
            }
        }
    }
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    tools: Vec<OpenAiTool>,
    tool_choice: &'static str,
    stream: bool,
}

#[derive(Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: OpenAiFunctionCall,
}

#[derive(Serialize, Deserialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiToolFunction,
}

#[derive(Serialize)]
struct OpenAiToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
    strict: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

fn trim_sse_line_end(line: &mut Vec<u8>) {
    if line.ends_with(b"\n") {
        line.pop();
    }
    if line.ends_with(b"\r") {
        line.pop();
    }
}

#[derive(Default)]
struct StreamedToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

fn handle_openai_stream_line(
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

fn convert_streamed_response(
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

fn convert_openai_response(response: ChatResponse) -> Result<ModelResponse, ModelError> {
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
        messages: Vec<Message>,
        token: &str,
        refresh_token: Option<&str>,
        account_id: Option<&str>,
        auth_path: Option<&std::path::Path>,
    ) -> Result<String, ModelError> {
        self.send_codex_responses_inner(messages, token, refresh_token, account_id, auth_path, None)
            .await
    }

    async fn send_codex_responses_stream(
        &self,
        messages: Vec<Message>,
        token: &str,
        refresh_token: Option<&str>,
        account_id: Option<&str>,
        auth_path: Option<&std::path::Path>,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<String, ModelError> {
        self.send_codex_responses_inner(
            messages,
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
        messages: Vec<Message>,
        token: &str,
        refresh_token: Option<&str>,
        account_id: Option<&str>,
        auth_path: Option<&std::path::Path>,
        mut on_event: Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
    ) -> Result<String, ModelError> {
        let mut instructions = Vec::new();
        let input: Vec<_> = messages
            .into_iter()
            .filter_map(|m| match m {
                Message::System(content) => {
                    instructions.push(content);
                    None
                }
                message => Some(json!({
                    "role": codex_role(&message),
                    "content": render_message_content(&message),
                })),
            })
            .collect();
        let instructions = instructions.join("\n\n");
        let url = format!("{}/responses", self.api_base.trim_end_matches('/'));
        let make_body = || {
            let mut body = json!({
                "model": self.model,
                "instructions": instructions,
                "input": input,
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

fn codex_reasoning_param(effort: Option<&str>, summary: Option<&str>) -> Option<serde_json::Value> {
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

fn to_openai_tool(tool: ToolSpec) -> OpenAiTool {
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

fn to_openai_message(message: Message) -> Result<OpenAiMessage, ModelError> {
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

fn codex_role(message: &Message) -> &'static str {
    match message {
        Message::Assistant(_) => "assistant",
        Message::System(_) | Message::User(_) | Message::ToolResult(_) => "user",
    }
}

fn render_message_content(message: &Message) -> String {
    match message {
        Message::System(content) => content.clone(),
        Message::User(blocks) | Message::Assistant(blocks) => render_blocks(blocks),
        Message::ToolResult(result) => format!(
            "Tool result for {} (ok={}):\n{}",
            result.id, result.ok, result.content
        ),
    }
}

fn render_tool_call(call: &ToolCall) -> String {
    let arguments = serde_json::to_string_pretty(&call.arguments)
        .unwrap_or_else(|_| call.arguments.to_string());
    format!("Tool call: {}\n{}", call.name, arguments)
}

#[derive(Deserialize)]
struct ResponsesResponse {
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
}

fn extract_response_text(response: ResponsesResponse) -> Result<String, ModelError> {
    if let Some(text) = response.output_text {
        return Ok(text);
    }
    let text = response
        .output
        .unwrap_or_default()
        .into_iter()
        .flat_map(|o| o.content.unwrap_or_default())
        .filter_map(|c| c.text)
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        Err(ModelError::InvalidResponse("missing response text".into()))
    } else {
        Ok(text)
    }
}

async fn collect_codex_sse_response(
    response: reqwest::Response,
    on_event: &mut Option<&mut dyn FnMut(ModelEvent) -> Result<(), ModelError>>,
) -> Result<String, ModelError> {
    let mut text = String::new();
    let mut completed_text = None;
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
            handle_codex_sse_line(line, &mut text, &mut completed_text, on_event)?;
        }
    }
    if !buffer.is_empty() {
        trim_sse_line_end(&mut buffer);
        let line = std::str::from_utf8(&buffer).map_err(|err| {
            ModelError::InvalidResponse(format!("streamed response contained invalid utf-8: {err}"))
        })?;
        handle_codex_sse_line(line, &mut text, &mut completed_text, on_event)?;
    }
    if !text.is_empty() {
        Ok(text)
    } else if let Some(text) = completed_text {
        Ok(text)
    } else {
        Err(ModelError::InvalidResponse(
            "missing response text in SSE".into(),
        ))
    }
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

fn handle_codex_sse_line(
    line: &str,
    text: &mut String,
    completed_text: &mut Option<String>,
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
            text.push_str(delta);
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::OutputDelta(delta.to_string()))?;
            }
        }
    } else if event_type.contains("reasoning") {
        if let Some(delta) = extract_reasoning_delta(&value) {
            if let Some(on_event) = on_event.as_mut() {
                on_event(ModelEvent::ReasoningDelta(delta))?;
            }
        }
    } else if event_type == "response.completed" && text.is_empty() {
        if let Ok(response) = serde_json::from_value::<ResponsesResponse>(value["response"].clone())
        {
            *completed_text = Some(extract_response_text(response)?);
        }
    }
    Ok(())
}

#[cfg(test)]
fn extract_sse_text(body: &str) -> Result<String, ModelError> {
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

#[derive(Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokens>,
}
#[derive(Deserialize)]
struct CodexTokens {
    access_token: String,
    refresh_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

fn load_codex_auth() -> Result<Auth, ModelError> {
    if let Ok(access_token) = std::env::var("CODEX_ACCESS_TOKEN") {
        let account_id = std::env::var("CODEX_ACCOUNT_ID").ok();
        return Ok(Auth::Codex {
            access_token,
            refresh_token: None,
            account_id,
            auth_path: None,
        });
    }
    let home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .map_err(|_| ModelError::MissingCodexAuth)?;
    let auth_path = home.join("auth.json");
    let tokens = load_codex_tokens_from_path(&auth_path)?;
    Ok(Auth::Codex {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        account_id: tokens.account_id,
        auth_path: Some(auth_path),
    })
}

fn load_codex_tokens_from_path(path: &std::path::Path) -> Result<CodexTokens, ModelError> {
    let text = std::fs::read_to_string(path).map_err(|_| ModelError::MissingCodexAuth)?;
    let file: CodexAuthFile = serde_json::from_str(&text)
        .map_err(|e| ModelError::InvalidResponse(format!("invalid Codex auth.json: {e}")))?;
    file.tokens.ok_or(ModelError::MissingCodexAuth)
}

async fn refresh_codex_token(
    client: &reqwest::Client,
    refresh_token: &str,
    auth_path: Option<&std::path::Path>,
) -> Result<CodexTokens, ModelError> {
    let response: RefreshResponse = client
        .post("https://auth.openai.com/oauth/token")
        .form(&[
            ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let access_token = response.access_token.ok_or_else(|| {
        ModelError::InvalidResponse("refresh response missing access_token".into())
    })?;
    let new_refresh_token = response
        .refresh_token
        .unwrap_or_else(|| refresh_token.to_string());
    let mut account_id = None;

    if let Some(path) = auth_path {
        let text = std::fs::read_to_string(path)?;
        let mut value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| ModelError::InvalidResponse(format!("invalid Codex auth.json: {e}")))?;
        if let Some(tokens) = value.get_mut("tokens") {
            if let Some(obj) = tokens.as_object_mut() {
                obj.insert("access_token".into(), json!(access_token));
                obj.insert("refresh_token".into(), json!(new_refresh_token));
                if let Some(id_token) = response.id_token {
                    obj.insert("id_token".into(), json!(id_token));
                }
                account_id = obj
                    .get("account_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
        }
        std::fs::write(path, serde_json::to_string_pretty(&value).unwrap())?;
    }

    Ok(CodexTokens {
        access_token,
        refresh_token: Some(new_refresh_token),
        account_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolResult;

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
        let mut text = String::new();
        let mut completed_text = None;
        let mut deltas = Vec::new();
        handle_codex_sse_line(
            r#"data: {"type":"response.output_text.delta","delta":"hi"}"#,
            &mut text,
            &mut completed_text,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::OutputDelta(delta) => deltas.push(delta),
                    ModelEvent::ReasoningDelta(_) => {}
                }
                Ok(())
            }),
        )
        .unwrap();

        assert_eq!(text, "hi");
        assert_eq!(deltas, vec!["hi"]);
        assert!(completed_text.is_none());
    }

    #[test]
    fn codex_sse_line_emits_reasoning_summary_delta() {
        let mut text = String::new();
        let mut completed_text = None;
        let mut deltas = Vec::new();
        handle_codex_sse_line(
            r#"data:{"type":"response.reasoning_summary_text.delta","delta":"thinking","summary_index":0}"#,
            &mut text,
            &mut completed_text,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::OutputDelta(_) => {}
                    ModelEvent::ReasoningDelta(delta) => deltas.push(delta),
                }
                Ok(())
            }),
        )
        .unwrap();

        assert!(text.is_empty());
        assert_eq!(deltas, vec!["thinking"]);
    }

    #[test]
    fn codex_sse_line_emits_reasoning_text_delta() {
        let mut text = String::new();
        let mut completed_text = None;
        let mut deltas = Vec::new();
        handle_codex_sse_line(
            r#"data: {"type":"response.reasoning_text.delta","delta":"raw","content_index":0}"#,
            &mut text,
            &mut completed_text,
            &mut Some(&mut |event| {
                match event {
                    ModelEvent::OutputDelta(_) => {}
                    ModelEvent::ReasoningDelta(delta) => deltas.push(delta),
                }
                Ok(())
            }),
        )
        .unwrap();

        assert!(text.is_empty());
        assert_eq!(deltas, vec!["raw"]);
    }

    #[test]
    fn extracts_completed_response_text_when_no_deltas() {
        let body = r#"data: {"type":"response.completed","response":{"output_text":"done","output":null}}
"#;
        assert_eq!(extract_sse_text(body).unwrap(), "done");
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
    fn renders_explicit_tool_history_as_text_for_codex_fallback() {
        let call = Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: json!({ "command": "pwd" }),
        })]);
        let result = Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "/tmp\n".into(),
        });
        assert!(render_message_content(&call).contains("Tool call: bash"));
        assert!(render_message_content(&result).contains("Tool result for call-1 (ok=true):"));
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
