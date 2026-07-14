use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Serialize)]
pub(crate) struct AnthropicRequest {
    pub(crate) model: String,
    pub(crate) max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) system: Option<Vec<AnthropicSystemBlock>>,
    pub(crate) messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_control: Option<AnthropicCacheControl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) thinking: Option<AnthropicThinkingConfig>,
    pub(crate) stream: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AnthropicThinkingConfig {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) budget_tokens: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AnthropicCacheControl {
    #[serde(rename = "type")]
    pub(crate) kind: String,
}

impl AnthropicCacheControl {
    pub(crate) fn ephemeral() -> Self {
        Self {
            kind: "ephemeral".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AnthropicSystemBlock {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_control: Option<AnthropicCacheControl>,
}

impl AnthropicSystemBlock {
    pub(crate) fn text(text: String, cache_control: Option<AnthropicCacheControl>) -> Self {
        Self {
            kind: "text",
            text,
            cache_control,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AnthropicMessage {
    pub(crate) role: AnthropicRole,
    pub(crate) content: Vec<AnthropicContentBlock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AnthropicRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<AnthropicCacheControl>,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<AnthropicCacheControl>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AnthropicImageSource {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) media_type: String,
    pub(crate) data: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct AnthropicTool {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_control: Option<AnthropicCacheControl>,
}

#[derive(Deserialize)]
pub(crate) struct AnthropicResponse {
    pub(crate) content: Vec<AnthropicContentBlock>,
    pub(crate) usage: Option<AnthropicUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct AnthropicUsage {
    #[serde(default)]
    pub(crate) input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) output_tokens: Option<u64>,
    #[serde(default, alias = "cache_read_tokens", alias = "cached_input_tokens")]
    pub(crate) cache_read_input_tokens: Option<u64>,
    #[serde(default, alias = "cache_write_tokens", alias = "cache_creation_tokens")]
    pub(crate) cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) cache_creation: Option<AnthropicCacheCreation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct AnthropicCacheCreation {
    #[serde(default, alias = "ephemeral_1h")]
    pub(crate) ephemeral_1h_input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) ephemeral_5m_input_tokens: Option<u64>,
}
