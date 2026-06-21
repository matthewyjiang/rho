use serde::{Deserialize, Serialize};
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
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("provider returned invalid response: {0}")]
    InvalidResponse(String),
}

#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;
}

pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    model: String,
}

impl OpenAiProvider {
    pub fn new(model: String, api_base: String) -> Result<Self, ModelError> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| ModelError::MissingApiKey)?;
        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
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

#[async_trait::async_trait]
impl ModelProvider for OpenAiProvider {
    async fn send_turn(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let ModelRequest {
            messages,
            tools: _tools,
        } = request;
        let messages = messages
            .into_iter()
            .map(|m| ChatMessage {
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
            })
            .collect();

        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response: ChatResponse = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let content = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::InvalidResponse("missing choices".into()))?
            .message
            .content;

        if let Some(call) = crate::prompt::parse_tool_call(&content)? {
            Ok(ModelResponse::ToolCall(call))
        } else {
            Ok(ModelResponse::FinalAnswer(content))
        }
    }
}
