use serde_json::Value;

use crate::protocol::openai_chat::{
    ChatReasoningReplay, ChatToolCallPolicy, OpenAiMessage, OpenAiTool,
};

#[path = "mistral_history.rs"]
mod mistral_history;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiCompatibleDialect {
    Standard,
    /// Config-defined Chat Completions hosts (vLLM, CLIProxyAPI, and similar).
    ///
    /// Unknown implementations often omit tool-call ids, use sparse indexes, or
    /// skip `{}` for zero-argument tools. Those quirks must not rewrite
    /// first-party dialects.
    Custom,
    /// Qwen Token Plan and similar hosts with lenient chat tool-call wire quirks.
    QwenTokenPlan,
    Poolside,
    OpenRouter,
    Moonshot,
    KimiCode,
    /// Mistral La Plateforme. It rejects undeclared request fields and carries
    /// reasoning as `thinking` content chunks instead of `reasoning_content`.
    Mistral,
}

impl OpenAiCompatibleDialect {
    /// How aggressively to normalize incomplete chat tool-call payloads.
    pub(crate) fn chat_tool_call_policy(self) -> ChatToolCallPolicy {
        match self {
            Self::Custom | Self::QwenTokenPlan => ChatToolCallPolicy::Lenient,
            Self::Standard
            | Self::Poolside
            | Self::OpenRouter
            | Self::Moonshot
            | Self::KimiCode
            | Self::Mistral => ChatToolCallPolicy::Strict,
        }
    }

    /// Whether chat requests carry OpenAI's `prompt_cache_key` extension.
    ///
    /// Config-defined hosts can be strict schema validators (Groq returns 400
    /// for any undeclared field), so they do not get it. Custom hosts that
    /// need a session cache key can speak Responses, where the field is part
    /// of the request shape.
    pub(crate) fn sends_prompt_cache_key(self) -> bool {
        match self {
            Self::Custom => false,
            Self::Standard
            | Self::QwenTokenPlan
            | Self::Poolside
            | Self::OpenRouter
            | Self::Moonshot
            | Self::KimiCode
            | Self::Mistral => true,
        }
    }

    /// Which field replayed reasoning goes back in.
    ///
    /// Config-defined hosts can be strict validators (Groq streams `reasoning`
    /// and rejects `reasoning_content`), so they get the field they streamed.
    /// Built-in hosts keep the `reasoning_content` they have always been sent.
    pub(crate) fn reasoning_replay(self) -> ChatReasoningReplay {
        match self {
            Self::Custom => ChatReasoningReplay::AsStreamed,
            Self::Standard
            | Self::QwenTokenPlan
            | Self::Poolside
            | Self::OpenRouter
            | Self::Moonshot
            | Self::KimiCode
            | Self::Mistral => ChatReasoningReplay::ReasoningContent,
        }
    }

    /// Rewrites converted chat history into the shape this host accepts.
    pub(crate) fn normalize_messages(self, messages: &mut [OpenAiMessage]) {
        match self {
            Self::Mistral => mistral_history::normalize(messages),
            Self::Standard
            | Self::Custom
            | Self::QwenTokenPlan
            | Self::Poolside
            | Self::OpenRouter
            | Self::Moonshot
            | Self::KimiCode => {}
        }
    }

    pub(crate) fn normalize_tool(self, mut tool: OpenAiTool<'_>) -> OpenAiTool<'_> {
        match self {
            Self::Standard
            | Self::Custom
            | Self::QwenTokenPlan
            | Self::Poolside
            | Self::OpenRouter
            | Self::Mistral => tool,
            Self::Moonshot | Self::KimiCode => {
                normalize_moonshot_parameters(tool.function.parameters.to_mut());
                tool
            }
        }
    }
}

fn normalize_moonshot_parameters(parameters: &mut Value) {
    let Some(object) = parameters.as_object_mut() else {
        return;
    };

    // Moonshot requires function parameters to be an object, but rejects a
    // root object type combined with anyOf. Keep the required root type and
    // rely on tool argument validation for the root alternatives.
    object.insert("type".into(), Value::String("object".into()));
    object.remove("anyOf");
    for value in object.values_mut() {
        normalize_moonshot_schema(value);
    }
}

fn normalize_moonshot_schema(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };

    let parent_type = object.get("type").cloned();
    let can_move_parent_type = parent_type.as_ref().is_some_and(|parent_type| {
        object
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|branches| {
                branches.iter().all(|branch| {
                    branch.as_object().is_some_and(|branch| {
                        branch.get("type").is_none_or(|kind| kind == parent_type)
                    })
                })
            })
    });
    if can_move_parent_type {
        let parent_type = parent_type.expect("checked above");
        if let Some(branches) = object.get_mut("anyOf").and_then(Value::as_array_mut) {
            for branch in branches {
                if let Some(branch) = branch.as_object_mut() {
                    branch.entry("type").or_insert_with(|| parent_type.clone());
                }
            }
        }
        object.remove("type");
    }

    for value in object.values_mut() {
        match value {
            Value::Array(values) => {
                for value in values {
                    normalize_moonshot_schema(value);
                }
            }
            Value::Object(_) => normalize_moonshot_schema(value),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "openai_compatible_dialect_tests.rs"]
mod tests;
