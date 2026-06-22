use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(super) struct ChatRequest {
    pub(super) model: String,
    pub(super) messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<&'static str>,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<ChatStreamOptions>,
}

#[derive(Serialize)]
pub(super) struct ChatStreamOptions {
    pub(super) include_usage: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct OpenAiMessage {
    pub(super) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct OpenAiToolCall {
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) function: OpenAiFunctionCall,
}

#[derive(Serialize, Deserialize)]
pub(super) struct OpenAiFunctionCall {
    pub(super) name: String,
    pub(super) arguments: String,
}

#[derive(Serialize)]
pub(super) struct OpenAiTool {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) function: OpenAiToolFunction,
}

#[derive(Serialize)]
pub(super) struct OpenAiToolFunction {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) parameters: serde_json::Value,
    pub(super) strict: bool,
}

#[derive(Deserialize)]
pub(super) struct ChatResponse {
    pub(super) choices: Vec<Choice>,
}

#[derive(Deserialize)]
pub(super) struct Choice {
    pub(super) message: ChatResponseMessage,
}

#[derive(Deserialize)]
pub(super) struct ChatResponseMessage {
    pub(super) content: Option<String>,
    pub(super) tool_calls: Option<Vec<OpenAiToolCall>>,
}
