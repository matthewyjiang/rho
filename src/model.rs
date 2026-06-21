use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::tool::{ToolCall, ToolSpec};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
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
        account_id: Option<String>,
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
                account_id,
            } => {
                self.send_codex_responses(messages, access_token, account_id.as_deref())
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
        account_id: Option<&str>,
    ) -> Result<String, ModelError> {
        let input: Vec<_> = messages.into_iter().map(|m| json!({
            "role": match m.role { Role::System => "system", Role::User => "user", Role::Assistant => "assistant", Role::Tool => "user" },
            "content": match m.role { Role::Tool => format!("Tool result:\n{}", m.content), _ => m.content },
        })).collect();
        let url = format!("{}/responses", self.api_base.trim_end_matches('/'));
        let mut req = self
            .client
            .post(url)
            .bearer_auth(token)
            .header("User-Agent", "codex-cli")
            .header("originator", "codex_cli_rs")
            .json(&json!({ "model": self.model, "input": input, "store": false }));
        if let Some(account_id) = account_id {
            req = req.header("ChatGPT-Account-ID", account_id);
        }
        let response: ResponsesResponse = req.send().await?.error_for_status()?.json().await?;
        extract_response_text(response)
    }
}

fn to_chat_message(m: Message) -> ChatMessage {
    ChatMessage {
        role: match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "user",
        }
        .to_string(),
        content: match m.role {
            Role::Tool => format!("Tool result:\n{}", m.content),
            _ => m.content,
        },
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

#[derive(Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokens>,
}
#[derive(Deserialize)]
struct CodexTokens {
    access_token: String,
    account_id: Option<String>,
}

fn load_codex_auth() -> Result<Auth, ModelError> {
    if let Ok(access_token) = std::env::var("CODEX_ACCESS_TOKEN") {
        let account_id = std::env::var("CODEX_ACCOUNT_ID").ok();
        return Ok(Auth::Codex {
            access_token,
            account_id,
        });
    }
    let home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .map_err(|_| ModelError::MissingCodexAuth)?;
    let text = std::fs::read_to_string(home.join("auth.json"))
        .map_err(|_| ModelError::MissingCodexAuth)?;
    let file: CodexAuthFile = serde_json::from_str(&text)
        .map_err(|e| ModelError::InvalidResponse(format!("invalid Codex auth.json: {e}")))?;
    let tokens = file.tokens.ok_or(ModelError::MissingCodexAuth)?;
    Ok(Auth::Codex {
        access_token: tokens.access_token,
        account_id: tokens.account_id,
    })
}
