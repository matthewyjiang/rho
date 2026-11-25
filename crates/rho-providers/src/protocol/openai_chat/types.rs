use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct ChatRequest<'a> {
    pub(crate) model: String,
    pub(crate) messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<OpenAiTool<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parallel_tool_calls: Option<bool>,
    pub(crate) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stream_options: Option<ChatStreamOptions>,
    /// Session-scoped prompt cache identity. Omitted when the caller has none.
    ///
    /// OpenAI Chat Completions and several compatible hosts use this to pin
    /// cache across turns. Strict hosts reject it as an undeclared field, so
    /// OpenAI-compatible dialects opt in through
    /// `OpenAiCompatibleDialect::sends_prompt_cache_key`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_cache_key: Option<String>,
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
#[serde(untagged)]
pub(crate) enum OpenAiReasoning {
    Effort { effort: String },
    Enabled { enabled: bool },
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

#[derive(Serialize)]
pub(crate) struct OpenAiMessage {
    pub(crate) role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content: Option<serde_json::Value>,
    /// Raw thinking retained for multi-turn tool loops, under the host's field.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning: Option<ChatReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
}

/// Message field a chat host uses for raw assistant reasoning.
///
/// Hosts disagree on the name: Qwen and DeepSeek stream `reasoning_content`,
/// Groq streams `reasoning`. Strict hosts reject names they do not declare,
/// so replayed reasoning goes back under the field it streamed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChatReasoningField {
    #[default]
    ReasoningContent,
    Reasoning,
    ReasoningText,
}

impl ChatReasoningField {
    /// Read order when a message or delta carries more than one name.
    pub(crate) const ALL: [Self; 3] =
        [Self::ReasoningContent, Self::Reasoning, Self::ReasoningText];

    pub(crate) fn wire_name(self) -> &'static str {
        match self {
            Self::ReasoningContent => "reasoning_content",
            Self::Reasoning => "reasoning",
            Self::ReasoningText => "reasoning_text",
        }
    }

    /// The first non-empty reasoning text in a chat message or delta object.
    pub(crate) fn find_in(value: &serde_json::Value) -> Option<(Self, &str)> {
        Self::ALL.into_iter().find_map(|field| {
            value
                .get(field.wire_name())
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .map(|text| (field, text))
        })
    }
}

/// Raw assistant reasoning and the message field it travels in.
///
/// Serializes as the single entry `{ <field>: <text> }`, flattened into
/// [`OpenAiMessage`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChatReasoning {
    pub(crate) field: ChatReasoningField,
    pub(crate) text: String,
}

impl Serialize for ChatReasoning {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.field.wire_name(), &self.text)?;
        map.end()
    }
}

/// Which message field a host gets replayed reasoning in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChatReasoningReplay {
    /// The field the reasoning streamed in. Config-defined hosts can be strict
    /// validators (Groq streams `reasoning` and rejects `reasoning_content`).
    AsStreamed,
    /// Always `reasoning_content`, the field built-in hosts have always been
    /// sent. DeepSeek V4 requires that name on tool-call turns.
    ReasoningContent,
}

impl ChatReasoningReplay {
    /// The field reasoning that streamed in `streamed` is replayed under.
    pub(crate) fn field(self, streamed: ChatReasoningField) -> ChatReasoningField {
        match self {
            Self::AsStreamed => streamed,
            Self::ReasoningContent => ChatReasoningField::ReasoningContent,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct OpenAiToolCall {
    /// Some OpenAI-compatible hosts omit ids; lenient dialects synthesize them.
    #[serde(default)]
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
pub(crate) struct OpenAiTool<'a> {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) function: OpenAiToolFunction<'a>,
}

#[derive(Serialize)]
pub(crate) struct OpenAiToolFunction<'a> {
    pub(crate) name: &'a str,
    pub(crate) description: &'a str,
    pub(crate) parameters: std::borrow::Cow<'a, serde_json::Value>,
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
    /// A string, or Mistral-style text and thinking chunks; see
    /// [`crate::protocol::openai_shared::content_chunks`].
    pub(crate) content: Option<serde_json::Value>,
    /// Qwen/DeepSeek-style raw thinking on non-stream completions.
    #[serde(default)]
    pub(crate) reasoning_content: Option<String>,
    pub(crate) tool_calls: Option<Vec<OpenAiToolCall>>,
}
