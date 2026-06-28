use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Serialize)]
pub(super) struct AnthropicRequest {
    pub(super) model: String,
    pub(super) max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) system: Option<Vec<AnthropicSystemBlock>>,
    pub(super) messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cache_control: Option<AnthropicCacheControl>,
    pub(super) stream: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct AnthropicCacheControl {
    #[serde(rename = "type")]
    pub(super) kind: String,
}

impl AnthropicCacheControl {
    pub(super) fn ephemeral() -> Self {
        Self {
            kind: "ephemeral".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct AnthropicSystemBlock {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cache_control: Option<AnthropicCacheControl>,
}

impl AnthropicSystemBlock {
    pub(super) fn text(text: String, cache_control: Option<AnthropicCacheControl>) -> Self {
        Self {
            kind: "text",
            text,
            cache_control,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct AnthropicMessage {
    pub(super) role: AnthropicRole,
    pub(super) content: Vec<AnthropicContentBlock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum AnthropicRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(super) enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<AnthropicCacheControl>,
    },
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
pub(super) struct AnthropicImageSource {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) media_type: String,
    pub(super) data: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct AnthropicTool {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cache_control: Option<AnthropicCacheControl>,
}

#[derive(Deserialize)]
pub(super) struct AnthropicResponse {
    pub(super) content: Vec<AnthropicContentBlock>,
    pub(super) usage: Option<AnthropicUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct AnthropicUsage {
    #[serde(default)]
    pub(super) input_tokens: Option<u64>,
    #[serde(default)]
    pub(super) output_tokens: Option<u64>,
    #[serde(default, alias = "cache_read_tokens", alias = "cached_input_tokens")]
    pub(super) cache_read_input_tokens: Option<u64>,
    #[serde(default, alias = "cache_write_tokens", alias = "cache_creation_tokens")]
    pub(super) cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub(super) cache_creation: Option<AnthropicCacheCreation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct AnthropicCacheCreation {
    #[serde(default, alias = "ephemeral_1h")]
    pub(super) ephemeral_1h_input_tokens: Option<u64>,
    #[serde(default)]
    pub(super) ephemeral_5m_input_tokens: Option<u64>,
}
