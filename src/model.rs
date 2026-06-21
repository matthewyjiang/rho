use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::tool::{ToolCall, ToolResult, ToolSpec};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Message {
    System(String),
    User(String),
    Assistant(String),
    AssistantToolCall(ToolCall),
    ToolResult(ToolResult),
}

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Clone, Debug)]
pub enum ModelResponse {
    FinalAnswer(String),
    ToolCall(ToolCall),
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("missing OPENAI_API_KEY")]
    MissingApiKey,
    #[error("missing Codex OAuth credentials; run `codex login` or set CODEX_ACCESS_TOKEN")]
    MissingCodexAuth,
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("request failed: HTTP {status}: {body}")]
    HttpStatus {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider returned invalid response: {0}")]
    InvalidResponse(String),
}

#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;
}

#[derive(Clone, Debug)]
pub enum AuthMode {
    ApiKey,
    Codex,
}

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

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
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

#[async_trait::async_trait]
impl ModelProvider for OpenAiProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let ModelRequest {
            messages,
            tools: _tools,
        } = request;
        let content = match &self.auth {
            Auth::ApiKey(key) => self.send_chat_completions(messages, key).await?,
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
                    messages,
                    &access_token,
                    refresh_token.as_deref(),
                    account_id.as_deref(),
                    auth_path_for_request.as_deref(),
                )
                .await?
            }
        };

        if let Some(call) = crate::prompt::parse_tool_call(&content)? {
            Ok(ModelResponse::ToolCall(call))
        } else {
            Ok(ModelResponse::FinalAnswer(content))
        }
    }
}

impl OpenAiProvider {
    async fn send_chat_completions(
        &self,
        messages: Vec<Message>,
        key: &str,
    ) -> Result<String, ModelError> {
        let messages = messages.into_iter().map(to_chat_message).collect();
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response: ChatResponse = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::InvalidResponse("missing choices".into()))?
            .message
            .content)
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

fn to_chat_message(message: Message) -> ChatMessage {
    ChatMessage {
        role: chat_role(&message).to_string(),
        content: render_message_content(&message),
    }
}

fn chat_role(message: &Message) -> &'static str {
    match message {
        Message::System(_) => "system",
        Message::User(_) => "user",
        Message::Assistant(_) | Message::AssistantToolCall(_) => "assistant",
        Message::ToolResult(_) => "user",
    }
}

fn codex_role(message: &Message) -> &'static str {
    match message {
        Message::Assistant(_) | Message::AssistantToolCall(_) => "assistant",
        Message::System(_) | Message::User(_) | Message::ToolResult(_) => "user",
    }
}

fn render_message_content(message: &Message) -> String {
    match message {
        Message::System(content) | Message::User(content) | Message::Assistant(content) => {
            content.clone()
        }
        Message::AssistantToolCall(call) => {
            let arguments = serde_json::to_string_pretty(&call.arguments)
                .unwrap_or_else(|_| call.arguments.to_string());
            format!("Tool call: {}\n{}", call.name, arguments)
        }
        Message::ToolResult(result) => format!(
            "Tool result for {} (ok={}):\n{}",
            result.id, result.ok, result.content
        ),
    }
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

    #[test]
    fn extracts_sse_delta_text() {
        let body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\n",
            "data: [DONE]\n"
        );

        let text = extract_sse_text(body).unwrap();

        assert_eq!(text, "Hello world");
    }

    #[test]
    fn extracts_completed_response_text_when_no_deltas() {
        let body = r#"data: {"type":"response.completed","response":{"output_text":"done","output":null}}
"#;

        let text = extract_sse_text(body).unwrap();

        assert_eq!(text, "done");
    }

    #[test]
    fn renders_explicit_tool_history_as_text() {
        let call = Message::AssistantToolCall(ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: json!({ "command": "pwd" }),
        });
        let result = Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "/tmp\n".into(),
        });

        let rendered_call = to_chat_message(call);
        let rendered_result = to_chat_message(result);

        assert_eq!(rendered_call.role, "assistant");
        assert!(rendered_call.content.contains("Tool call: bash"));
        assert!(rendered_call.content.contains("\"command\": \"pwd\""));
        assert_eq!(rendered_result.role, "user");
        assert!(rendered_result
            .content
            .contains("Tool result for call-1 (ok=true):"));
    }
}
