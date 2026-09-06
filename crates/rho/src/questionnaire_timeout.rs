use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{QuestionnaireQuestion, QuestionnaireQuestionKind};

/// Explicit model-proposed fallback, independent of preselected defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionnaireTimeout {
    pub answers: BTreeMap<String, TimeoutAnswer>,
    pub reason: String,
}

/// Wire answer shapes accepted by a timeout fallback. Question-specific
/// validation still enforces selection kind, allowed values, and requiredness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TimeoutAnswer {
    Text(String),
    Confirm(bool),
    Selections(Vec<String>),
}

impl QuestionnaireTimeout {
    pub(super) fn validate(&self, questions: &[QuestionnaireQuestion]) -> Result<(), String> {
        if self.reason.trim().is_empty() {
            return Err("on_timeout.reason must not be empty".into());
        }
        for id in self.answers.keys() {
            if !questions.iter().any(|question| &question.id == id) {
                return Err(format!(
                    "on_timeout.answers contains unknown question '{id}'"
                ));
            }
        }
        for question in questions {
            let id = &question.id;
            let Some(answer) = self.answers.get(id) else {
                if question.required {
                    return Err(format!(
                        "on_timeout.answers is missing required question '{id}'"
                    ));
                }
                continue;
            };
            let valid_choice = |value: &str| {
                !value.trim().is_empty()
                    && (question.allow_other
                        || question.choices.iter().any(|choice| choice.label == value))
            };
            let valid = match (question.kind, answer) {
                (QuestionnaireQuestionKind::Confirm, TimeoutAnswer::Confirm(_)) => true,
                (QuestionnaireQuestionKind::Choice, TimeoutAnswer::Text(value)) => {
                    valid_choice(value)
                }
                (QuestionnaireQuestionKind::MultiSelect, TimeoutAnswer::Selections(values)) => {
                    let mut unique = BTreeSet::new();
                    (!question.required || !values.is_empty())
                        && values
                            .iter()
                            .all(|value| valid_choice(value) && unique.insert(value))
                }
                (QuestionnaireQuestionKind::Text, TimeoutAnswer::Text(value)) => {
                    !value.trim().is_empty()
                }
                _ => false,
            };
            if !valid {
                return Err(format!("on_timeout.answers['{id}'] has an invalid type, choice, or empty required answer"));
            }
        }
        Ok(())
    }

    pub(crate) fn host_response(&self) -> rho_sdk::HostInputResponse {
        self.answers.iter().fold(
            rho_sdk::HostInputResponse::new(),
            |response, (id, value)| {
                let values = match value {
                    TimeoutAnswer::Confirm(value) => vec![if *value { "yes" } else { "no" }.into()],
                    TimeoutAnswer::Text(value) => vec![value.clone()],
                    TimeoutAnswer::Selections(values) => values.clone(),
                };
                response.answer(id, values)
            },
        )
    }
}

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["answers", "reason"],
        "description": "Explicit fallback for safe, reversible decisions only, never authorization. Requires explicit question ids and answers for every required question. The user's configuration controls whether and when it runs.",
        "properties": {
            "answers": {
                "type": "object",
                "additionalProperties": { "oneOf": [
                    {"type": "string"}, {"type": "boolean"},
                    {"type": "array", "items": {"type": "string"}}
                ]},
                "description": "Question id to fallback answer. Use exact choice labels, arrays for multi_select, booleans for confirm. Optional questions may be omitted."
            },
            "reason": {"type": "string", "description": "Explain the consequence of proceeding with these fallback answers."}
        }
    })
}
