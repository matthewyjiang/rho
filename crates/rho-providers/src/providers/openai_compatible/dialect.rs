use serde_json::Value;

use crate::{
    protocol::openai_chat::{OpenAiReasoning, OpenAiThinking, OpenAiTool},
    reasoning::ReasoningLevel,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenAiCompatibleDialect {
    OpenRouter,
    Moonshot,
    KimiCode,
}

pub(crate) struct OpenAiCompatibleReasoningFields {
    pub(crate) reasoning: Option<OpenAiReasoning>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) thinking: Option<OpenAiThinking>,
}

impl OpenAiCompatibleDialect {
    pub(crate) fn normalize_tool(self, mut tool: OpenAiTool) -> OpenAiTool {
        match self {
            Self::OpenRouter => tool,
            Self::Moonshot | Self::KimiCode => {
                normalize_moonshot_parameters(&mut tool.function.parameters);
                tool
            }
        }
    }

    pub(crate) fn reasoning_fields(
        self,
        model: &str,
        reasoning: ReasoningLevel,
    ) -> OpenAiCompatibleReasoningFields {
        let empty = || OpenAiCompatibleReasoningFields {
            reasoning: None,
            reasoning_effort: None,
            thinking: None,
        };
        match (self, model) {
            (Self::OpenRouter, _) => OpenAiCompatibleReasoningFields {
                reasoning: Some(OpenAiReasoning {
                    effort: reasoning.effort().unwrap_or("none").to_string(),
                }),
                ..empty()
            },
            (Self::Moonshot, "kimi-k3") => OpenAiCompatibleReasoningFields {
                reasoning_effort: Some(reasoning.effort().unwrap_or("none").to_string()),
                ..empty()
            },
            (Self::KimiCode, "k3") => OpenAiCompatibleReasoningFields {
                thinking: Some(match reasoning {
                    ReasoningLevel::Off => OpenAiThinking {
                        kind: "disabled",
                        effort: None,
                    },
                    ReasoningLevel::Minimal => enabled_thinking("minimal"),
                    ReasoningLevel::Low => enabled_thinking("low"),
                    ReasoningLevel::Medium => enabled_thinking("medium"),
                    ReasoningLevel::High => enabled_thinking("high"),
                    ReasoningLevel::Xhigh => enabled_thinking("xhigh"),
                    ReasoningLevel::Max => enabled_thinking("max"),
                }),
                ..empty()
            },
            (Self::Moonshot | Self::KimiCode, _) => empty(),
        }
    }
}

fn enabled_thinking(effort: &str) -> OpenAiThinking {
    OpenAiThinking {
        kind: "enabled",
        effort: Some(effort.to_string()),
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
#[path = "dialect_tests.rs"]
mod tests;
