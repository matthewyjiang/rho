use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::{
    AuthMode, ContentBlock, Message, ModelError, ModelProvider, ModelRequest, ModelResponse,
};
use crate::tool::{ToolCall, ToolSpec};

pub struct OpenAiProvider {
    client: reqwest::Client,
    auth: Auth,
    api_base: String,
    model: String,
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
    pub fn new(model: String, api_base: String, mode: AuthMode) -> Result<Self, ModelError> {
        let auth = match mode {
            AuthMode::ApiKey => Auth::ApiKey(
                std::env::var("OPENAI_API_KEY").map_err(|_| ModelError::MissingApiKey)?,
            ),
            AuthMode::Codex => load_codex_auth()?,
        };
        let api_base = match auth {
            Auth::Codex { .. } if api_base == "https://api.openai.com/v1" => {
                "https://chatgpt.com/backend-api/codex".into()
            }
            _ => api_base,
        };
        Ok(Self {
            client: reqwest::Client::new(),
            auth,
            api_base,
            model,
        })
    }
}

#[async_trait::async_trait]
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
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    tools: Vec<OpenAiTool>,
    tool_choice: &'static str,
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
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
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

    async fn send_codex_responses(
        &self,
        messages: Vec<Message>,
        token: &str,
        refresh_token: Option<&str>,
        account_id: Option<&str>,
        auth_path: Option<&std::path::Path>,
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
        let make_request = |token: &str| {
            self.client
                .post(&url)
                .bearer_auth(token)
                .header("User-Agent", "codex-cli")
                .header("originator", "codex_cli_rs")
                .json(&json!({ "model": self.model, "instructions": instructions, "input": input, "store": false, "stream": true }))
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
                let body = response.text().await?;
                return extract_sse_text(&body);
            }
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
        }
        let body = response.text().await?;
        extract_sse_text(&body)
    }
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

fn extract_sse_text(body: &str) -> Result<String, ModelError> {
    let mut text = String::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
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
    fn extracts_completed_response_text_when_no_deltas() {
        let body = r#"data: {"type":"response.completed","response":{"output_text":"done","output":null}}
"#;
        assert_eq!(extract_sse_text(body).unwrap(), "done");
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
