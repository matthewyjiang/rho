use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use rho_tools::tool::ToolSpec;

pub const TOOL_NAME: &str = "questionnaire";
const MAX_QUESTIONS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    pub questions: Vec<QuestionnaireQuestion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
    /// Whether `default` pre-selects the answer or only focuses it.
    #[serde(default, skip_serializing_if = "is_default_selection_selected")]
    pub default_selection: QuestionnaireDefaultSelection,
    #[serde(rename = "type", alias = "kind", default)]
    pub kind: QuestionnaireQuestionKind,
    #[serde(default = "default_required")]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<QuestionnaireChoice>,
    #[serde(default)]
    pub allow_other: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawQuestionnaireChoice")]
pub struct QuestionnaireChoice {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl From<RawQuestionnaireChoice> for QuestionnaireChoice {
    fn from(choice: RawQuestionnaireChoice) -> Self {
        match choice {
            RawQuestionnaireChoice::Label(label) => label.into(),
            RawQuestionnaireChoice::Detailed(choice) => Self {
                label: choice.label,
                description: choice.description,
            },
        }
    }
}

impl From<String> for QuestionnaireChoice {
    fn from(label: String) -> Self {
        Self {
            label,
            description: None,
        }
    }
}

impl From<&str> for QuestionnaireChoice {
    fn from(label: &str) -> Self {
        label.to_string().into()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionnaireQuestionKind {
    #[default]
    Text,
    Choice,
    MultiSelect,
    Confirm,
}

/// How a question's `default` should be applied in the host UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionnaireDefaultSelection {
    /// Pre-select the default answer.
    #[default]
    Selected,
    /// Focus the default choice without selecting it. Hosts may mark it as
    /// recommended so the user still confirms consciously.
    Focused,
}

fn is_default_selection_selected(value: &QuestionnaireDefaultSelection) -> bool {
    matches!(value, QuestionnaireDefaultSelection::Selected)
}

impl From<QuestionnaireDefaultSelection> for rho_sdk::DefaultSelection {
    fn from(value: QuestionnaireDefaultSelection) -> Self {
        match value {
            QuestionnaireDefaultSelection::Selected => Self::Selected,
            QuestionnaireDefaultSelection::Focused => Self::Focused,
        }
    }
}

impl From<rho_sdk::DefaultSelection> for QuestionnaireDefaultSelection {
    fn from(value: rho_sdk::DefaultSelection) -> Self {
        match value {
            rho_sdk::DefaultSelection::Selected => Self::Selected,
            rho_sdk::DefaultSelection::Focused => Self::Focused,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireResponse {
    pub answers: Vec<QuestionnaireAnswer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireAnswer {
    pub id: String,
    pub answer: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQuestionnaireRequest {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    questions: Vec<RawQuestionnaireQuestion>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    default: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQuestionnaireQuestion {
    #[serde(default)]
    id: Option<String>,
    question: String,
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    help: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(rename = "type", alias = "kind", default)]
    kind: Option<QuestionnaireQuestionKind>,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    default_selection: QuestionnaireDefaultSelection,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    choices: Vec<QuestionnaireChoice>,
    #[serde(default)]
    allow_other: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawQuestionnaireChoice {
    Label(String),
    Detailed(RawDetailedChoice),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDetailedChoice {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

pub fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: TOOL_NAME.into(),
        description: "Ask a concise selection form only when missing input blocks correctness, safety, or requested preferences. Main mode: concrete choice or multi_select questions with allow_other when needed. Avoid free-text fields unless truly unavoidable.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["questions"],
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Optional short form title."
                },
                "reason": {
                    "type": "string",
                    "description": "Optional brief explanation of why these answers are needed."
                },
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_QUESTIONS,
                    "description": "The main form. Include every related missing input here. Use a one-item array only when exactly one answer is needed.",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["question", "type"],
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Optional stable key for the answer, such as file, language, or confirm_overwrite. If omitted, q1, q2, etc. are assigned."
                            },
                            "question": {
                                "type": "string",
                                "description": "The concise question or field label to show the user."
                            },
                            "header": {
                                "type": "string",
                                "maxLength": 16,
                                "description": "Very short label (max 16 chars, e.g. Branch, Test suites) shown on this question's tab in multi-question forms. Defaults to the question text."
                            },
                            "help": {
                                "type": "string",
                                "description": "Optional short per-question help text."
                            },
                            "type": {
                                "type": "string",
                                "enum": ["choice", "multi_select", "confirm"],
                                "description": "Question type. Use choice for one answer, multi_select for several, confirm for yes/no. Defaults to choice when choices are provided."
                            },
                            "choices": {
                                "type": "array",
                                "items": {
                                    "oneOf": [
                                        { "type": "string" },
                                        {
                                            "type": "object",
                                            "additionalProperties": false,
                                            "required": ["label"],
                                            "properties": {
                                                "label": {
                                                    "type": "string",
                                                    "description": "The concise choice label returned as the answer."
                                                },
                                                "description": {
                                                    "type": "string",
                                                    "description": "Optional short detail shown below the label."
                                                }
                                            }
                                        }
                                    ]
                                },
                                "description": "Concrete answers the user can select from. Use {label, description} objects when a choice needs more detail. Plain strings remain supported. Required for choice and multi_select."
                            },
                            "allow_other": {
                                "type": "boolean",
                                "description": "Add an Other option so the user only types when none of the concrete choices fit."
                            },
                            "default": {
                                "description": "Optional default answer. For multi_select, use an array of selected choices. For confirm, use true/false or yes/no.",
                                "oneOf": [
                                    { "type": "string" },
                                    { "type": "boolean" },
                                    { "type": "number" },
                                    { "type": "array", "items": { "type": "string" } }
                                ]
                            },
                            "default_selection": {
                                "type": "string",
                                "enum": ["selected", "focused"],
                                "description": "How to apply default. selected pre-selects it. focused only moves the cursor and may mark the choice recommended, so the user still confirms. Requires default. Defaults to selected."
                            },
                            "required": {
                                "type": "boolean",
                                "description": "Whether a non-empty answer is required. Defaults to true."
                            }
                        }
                    }
                }
            }
        }),
    }
}

pub fn parse_request(arguments: Value) -> Result<QuestionnaireRequest, String> {
    let raw: RawQuestionnaireRequest = serde_json::from_value(arguments)
        .map_err(|err| format!("invalid questionnaire arguments: {err}"))?;
    let mut questions = raw.questions;
    let legacy_single_question = questions.is_empty() && raw.question.is_some();
    if questions.is_empty() {
        if let Some(question) = raw.question {
            questions.push(RawQuestionnaireQuestion {
                id: None,
                question,
                header: None,
                help: None,
                reason: None,
                kind: None,
                default: raw.default,
                default_selection: QuestionnaireDefaultSelection::Selected,
                required: None,
                choices: Vec::new(),
                allow_other: false,
            });
        }
    }
    if questions.is_empty() {
        return Err("questions must include at least one question".into());
    }
    if questions.len() > MAX_QUESTIONS {
        return Err(format!(
            "questions must include at most {MAX_QUESTIONS} questions"
        ));
    }

    let questions = questions
        .into_iter()
        .enumerate()
        .map(|(index, question)| parse_question(index, question, legacy_single_question))
        .collect::<Result<Vec<_>, _>>()?;
    ensure_unique_ids(&questions)?;
    Ok(QuestionnaireRequest {
        title: trim_optional(raw.title),
        reason: trim_optional(raw.reason),
        questions,
    })
}

pub fn response_content(response: &QuestionnaireResponse) -> String {
    serde_json::to_string(response).unwrap_or_else(|_| format!("answers: {:?}", response.answers))
}

pub fn start_display_lines(request: &QuestionnaireRequest) -> Vec<String> {
    let heading = request.title.as_ref().map_or_else(
        || TOOL_NAME.to_string(),
        |title| format!("{TOOL_NAME}: {title}"),
    );
    let mut lines = vec![heading];
    for (index, question) in request.questions.iter().enumerate() {
        lines.push(format!("{}. {}", index + 1, question.question));
    }
    lines
}

fn parse_question(
    index: usize,
    raw: RawQuestionnaireQuestion,
    allow_text: bool,
) -> Result<QuestionnaireQuestion, String> {
    let id = match trim_optional(raw.id) {
        Some(id) => id,
        None => format!("q{}", index + 1),
    };
    if !is_valid_id(&id) {
        return Err(format!(
            "questions[{index}].id must contain only ASCII letters, numbers, underscores, or dashes"
        ));
    }

    let question = raw.question.trim().to_string();
    if question.is_empty() {
        return Err(format!("questions[{index}].question must not be empty"));
    }

    let choices = raw
        .choices
        .into_iter()
        .filter_map(|choice| {
            trim_optional(Some(choice.label)).map(|label| QuestionnaireChoice {
                label,
                description: trim_optional(choice.description),
            })
        })
        .collect::<Vec<_>>();
    let kind = raw.kind.unwrap_or(if choices.is_empty() {
        QuestionnaireQuestionKind::Text
    } else {
        QuestionnaireQuestionKind::Choice
    });
    if matches!(kind, QuestionnaireQuestionKind::Text) && !allow_text {
        return Err(format!(
            "questions[{index}] must use choice, multi_select, or confirm"
        ));
    }
    if matches!(
        kind,
        QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect
    ) && choices.is_empty()
    {
        return Err(format!(
            "questions[{index}].choices must include at least one choice"
        ));
    }
    if !matches!(
        kind,
        QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect
    ) && !choices.is_empty()
    {
        return Err(format!(
            "questions[{index}].choices is only valid for choice or multi_select questions"
        ));
    }
    if raw.allow_other
        && !matches!(
            kind,
            QuestionnaireQuestionKind::Choice | QuestionnaireQuestionKind::MultiSelect
        )
    {
        return Err(format!(
            "questions[{index}].allow_other is only valid for choice or multi_select questions"
        ));
    }

    let default = normalize_default(index, raw.default, kind, &choices, raw.allow_other)?;
    if matches!(
        raw.default_selection,
        QuestionnaireDefaultSelection::Focused
    ) && default.is_none()
    {
        return Err(format!(
            "questions[{index}].default_selection focused requires default"
        ));
    }
    Ok(QuestionnaireQuestion {
        id,
        question,
        header: trim_optional(raw.header),
        help: trim_optional(raw.help).or_else(|| trim_optional(raw.reason)),
        default,
        default_selection: raw.default_selection,
        kind,
        required: raw.required.unwrap_or_else(default_required),
        choices,
        allow_other: raw.allow_other,
    })
}

fn normalize_default(
    index: usize,
    default: Option<Value>,
    kind: QuestionnaireQuestionKind,
    choices: &[QuestionnaireChoice],
    allow_other: bool,
) -> Result<Option<Value>, String> {
    let Some(default) = default else {
        return Ok(None);
    };
    if matches!(kind, QuestionnaireQuestionKind::MultiSelect) {
        let values = match default {
            Value::Null => return Ok(None),
            Value::Array(values) => values
                .into_iter()
                .map(|value| scalar_default_value(index, value, /*in_array*/ true))
                .collect::<Result<Vec<_>, _>>()?,
            value => vec![scalar_default_value(index, value, /*in_array*/ false)?],
        };
        let mut normalized = Vec::new();
        for value in values {
            let Some(value) = trim_optional(Some(value)) else {
                continue;
            };
            match choices
                .iter()
                .find(|choice| choice.label.eq_ignore_ascii_case(&value))
            {
                Some(choice) => normalized.push(Value::String(choice.label.clone())),
                None if allow_other => normalized.push(Value::String(value)),
                None => {
                    return Err(format!(
                        "questions[{index}].default must match one of the choices"
                    ));
                }
            }
        }
        return Ok((!normalized.is_empty()).then_some(Value::Array(normalized)));
    }

    let value = match default {
        Value::Null => return Ok(None),
        Value::Bool(value) if matches!(kind, QuestionnaireQuestionKind::Confirm) => {
            if value { "yes" } else { "no" }.into()
        }
        value => scalar_default_value(index, value, /*in_array*/ false)?,
    };
    let Some(value) = trim_optional(Some(value)) else {
        return Ok(None);
    };
    match kind {
        QuestionnaireQuestionKind::Text => Ok(Some(Value::String(value))),
        QuestionnaireQuestionKind::Choice => choices
            .iter()
            .find(|choice| choice.label.eq_ignore_ascii_case(&value))
            .map(|choice| choice.label.clone())
            .or_else(|| allow_other.then_some(value))
            .map(Value::String)
            .map(Some)
            .ok_or_else(|| format!("questions[{index}].default must match one of the choices")),
        QuestionnaireQuestionKind::MultiSelect => unreachable!("multi_select handled above"),
        QuestionnaireQuestionKind::Confirm => normalize_confirm_value(&value)
            .map(Value::String)
            .map(Some)
            .ok_or_else(|| format!("questions[{index}].default must be yes or no")),
    }
}

fn scalar_default_value(index: usize, value: Value, in_array: bool) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => {
            let location = if in_array { " array values" } else { "" };
            Err(format!(
                "questions[{index}].default{location} must be strings, booleans, or numbers"
            ))
        }
    }
}

fn normalize_confirm_value(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "y" | "true" => Some("yes".into()),
        "no" | "n" | "false" => Some("no".into()),
        _ => None,
    }
}

fn ensure_unique_ids(questions: &[QuestionnaireQuestion]) -> Result<(), String> {
    for (index, question) in questions.iter().enumerate() {
        if questions[..index]
            .iter()
            .any(|previous| previous.id == question.id)
        {
            return Err(format!("questions[{index}].id must be unique"));
        }
    }
    Ok(())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn default_required() -> bool {
    true
}

#[cfg(test)]
#[path = "questionnaire_tests.rs"]
mod tests;
