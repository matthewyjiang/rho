use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct ChatRequest {
    pub(crate) model: String,
    pub(crate) messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parallel_tool_calls: Option<bool>,
    pub(crate) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stream_options: Option<ChatStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning: Option<OpenAiReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) thinking: Option<OpenAiThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chat_template_kwargs: Option<ChatTemplateKwargs>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ChatTemplateKwargs {
    pub(crate) enable_thinking: bool,
}

#[derive(Serialize)]
pub(crate) struct OpenAiReasoning {
    pub(crate) effort: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct OpenAiThinking {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ChatStreamOptions {
    pub(crate) include_usage: bool,
}

/// Whether omitted reasoning-token details are likely hiding thinking tokens.
///
/// Derived from serialized request controls only. Absent controls are
/// [`Unlikely`]; an explicit thinking-on field is [`Likely`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HiddenReasoningRisk {
    #[default]
    Unlikely,
    Likely,
}

impl ChatRequest {
    pub(crate) fn hidden_reasoning_risk(&self) -> HiddenReasoningRisk {
        if let Some(kwargs) = self.chat_template_kwargs {
            return if kwargs.enable_thinking {
                HiddenReasoningRisk::Likely
            } else {
                HiddenReasoningRisk::Unlikely
            };
        }
        if let Some(thinking) = &self.thinking {
            return if thinking.kind == "disabled" {
                HiddenReasoningRisk::Unlikely
            } else {
                HiddenReasoningRisk::Likely
            };
        }
        let effort = self
            .reasoning
            .as_ref()
            .map(|reasoning| reasoning.effort.as_str())
            .or(self.reasoning_effort.as_deref());
        match effort {
            Some("none") | None => HiddenReasoningRisk::Unlikely,
            Some(_) => HiddenReasoningRisk::Likely,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct OpenAiMessage {
    pub(crate) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<serde_json::Value>,
    /// Qwen/DeepSeek-style raw thinking retained for multi-turn tool loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct OpenAiToolCall {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) function: OpenAiFunctionCall,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct OpenAiFunctionCall {
    pub(crate) name: String,
    pub(crate) arguments: String,
}

#[derive(Serialize)]
pub(crate) struct OpenAiTool {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) function: OpenAiToolFunction,
}

#[derive(Serialize)]
pub(crate) struct OpenAiToolFunction {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: serde_json::Value,
    pub(crate) strict: bool,
}

#[derive(Deserialize)]
pub(crate) struct ChatResponse {
    pub(crate) choices: Vec<Choice>,
}

#[derive(Deserialize)]
pub(crate) struct Choice {
    pub(crate) message: ChatResponseMessage,
}

#[derive(Deserialize)]
pub(crate) struct ChatResponseMessage {
    pub(crate) content: Option<String>,
    /// Qwen/DeepSeek-style raw thinking on non-stream completions.
    #[serde(default)]
    pub(crate) reasoning_content: Option<String>,
    pub(crate) tool_calls: Option<Vec<OpenAiToolCall>>,
}
