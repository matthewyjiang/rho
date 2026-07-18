use serde_json::Value;

use crate::{
    protocol::openai_chat::{OpenAiThinking, OpenAiTool},
    reasoning::ReasoningLevel,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenAiCompatibleDialect {
    OpenRouter,
    Moonshot,
}

impl OpenAiCompatibleDialect {
    pub(crate) fn normalize_tool(self, mut tool: OpenAiTool) -> OpenAiTool {
        match self {
            Self::OpenRouter => tool,
            Self::Moonshot => {
                normalize_moonshot_parameters(&mut tool.function.parameters);
                tool
            }
        }
    }

    pub(crate) fn reasoning(self, reasoning: ReasoningLevel) -> Option<String> {
        match self {
            Self::OpenRouter => Some(reasoning.effort().unwrap_or("none").to_string()),
            Self::Moonshot => None,
        }
    }

    pub(crate) fn thinking(
        self,
        provider: &str,
        model: &str,
        reasoning: ReasoningLevel,
    ) -> Option<OpenAiThinking> {
        let metadata_model = crate::provider::provider_descriptor(provider)
            .map(|descriptor| descriptor.metadata_model(model))
            .unwrap_or(model);
        match (self, metadata_model) {
            (Self::Moonshot, "kimi-k3") => Some(OpenAiThinking {
                kind: if reasoning == ReasoningLevel::Off {
                    "disabled"
                } else {
                    "enabled"
                },
            }),
            _ => None,
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
#[path = "dialect_tests.rs"]
mod tests;
