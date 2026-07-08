use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::ToolSpec;

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
    pub help: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(rename = "type", alias = "kind", default)]
    pub kind: QuestionnaireQuestionKind,
    #[serde(default = "default_required")]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub allow_other: bool,
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
    help: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(rename = "type", alias = "kind", default)]
    kind: Option<QuestionnaireQuestionKind>,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    choices: Vec<String>,
    #[serde(default)]
    allow_other: bool,
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
                                "items": { "type": "string" },
                                "description": "Concrete answers the user can select from. Required for choice and multi_select."
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
                help: None,
                reason: None,
                kind: None,
                default: raw.default,
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
    let mut lines = vec![TOOL_NAME.to_string()];
    if let Some(title) = &request.title {
        lines.push(title.clone());
    } else {
        lines.push(format!("{} question form", request.questions.len()));
    }
    if let Some(reason) = &request.reason {
        lines.push(format!("reason: {reason}"));
    }
    for (index, question) in request.questions.iter().enumerate() {
        lines.push(format!("{}. {}", index + 1, question.question));
        if let Some(help) = &question.help {
            lines.push(format!("   help: {help}"));
        }
        if let Some(default) = &question.default {
            lines.push(format!(
                "   default: {}",
                questionnaire_default_display(default)
            ));
        }
        if !question.choices.is_empty() {
            let suffix = if question.allow_other { ", other" } else { "" };
            lines.push(format!(
                "   choices: {}{suffix}",
                question.choices.join(", ")
            ));
        }
        if !question.required {
            lines.push("   optional".into());
        }
    }
    lines
}

pub fn finished_display_lines(request: &QuestionnaireRequest, result_content: &str) -> Vec<String> {
    let mut lines = start_display_lines(request);
    if !result_content.trim().is_empty() {
        lines.push(result_content.to_string());
    }
    lines
}

fn questionnaire_default_display(default: &Value) -> String {
    match default {
        Value::Array(values) => values
            .iter()
            .map(questionnaire_default_display)
            .collect::<Vec<_>>()
            .join(", "),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        Value::Object(_) => default.to_string(),
    }
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
        .filter_map(|choice| trim_optional(Some(choice)))
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
    Ok(QuestionnaireQuestion {
        id,
        question,
        help: trim_optional(raw.help).or_else(|| trim_optional(raw.reason)),
        default,
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
    choices: &[String],
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
                .find(|choice| choice.eq_ignore_ascii_case(&value))
            {
                Some(choice) => normalized.push(Value::String(choice.clone())),
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
            .find(|choice| choice.eq_ignore_ascii_case(&value))
            .cloned()
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
mod tests {
    use super::*;

    #[test]
    fn parse_request_trims_optional_fields() {
        let request = parse_request(json!({
            "title": "  Edit target  ",
            "reason": "  I need a target  ",
            "questions": [
                {
                    "id": " file ",
                    "question": "  Which file?  ",
                    "help": "  Use a repo-relative path  ",
                    "type": "choice",
                    "choices": ["src/main.rs", "src/lib.rs"],
                    "default": "  src/main.rs  "
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            request,
            QuestionnaireRequest {
                title: Some("Edit target".into()),
                reason: Some("I need a target".into()),
                questions: vec![QuestionnaireQuestion {
                    id: "file".into(),
                    question: "Which file?".into(),
                    help: Some("Use a repo-relative path".into()),
                    default: Some(json!("src/main.rs")),
                    kind: QuestionnaireQuestionKind::Choice,
                    required: true,
                    choices: vec!["src/main.rs".into(), "src/lib.rs".into()],
                    allow_other: false,
                }],
            }
        );
    }

    #[test]
    fn parse_request_accepts_legacy_single_question() {
        let request = parse_request(json!({
            "question": "  Which file?  ",
            "reason": "  I need a target  ",
            "default": "  src/main.rs  "
        }))
        .unwrap();

        assert_eq!(request.questions.len(), 1);
        assert_eq!(request.questions[0].id, "q1");
        assert_eq!(request.questions[0].question, "Which file?");
        assert_eq!(request.questions[0].default, Some(json!("src/main.rs")));
    }

    #[test]
    fn parse_request_normalizes_multi_select_defaults_and_other() {
        let request = parse_request(json!({
            "questions": [
                {
                    "id": "suites",
                    "question": "Which suites?",
                    "type": "multi_select",
                    "choices": ["unit", "e2e"],
                    "allow_other": true,
                    "default": ["Unit", "smoke"]
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            request.questions[0].kind,
            QuestionnaireQuestionKind::MultiSelect
        );
        assert!(request.questions[0].allow_other);
        assert_eq!(request.questions[0].default, Some(json!(["unit", "smoke"])));
    }

    #[test]
    fn parse_request_rejects_empty_questions() {
        let err = parse_request(json!({ "questions": [] })).unwrap_err();

        assert_eq!(err, "questions must include at least one question");
    }

    #[test]
    fn parse_request_rejects_text_questions_in_forms() {
        let err = parse_request(json!({
            "questions": [
                {
                    "id": "freeform",
                    "question": "What should I do?"
                }
            ]
        }))
        .unwrap_err();

        assert_eq!(
            err,
            "questions[0] must use choice, multi_select, or confirm"
        );
    }

    #[test]
    fn parse_request_normalizes_choice_and_confirm_defaults() {
        let request = parse_request(json!({
            "questions": [
                {
                    "id": "style",
                    "question": "Style?",
                    "type": "choice",
                    "choices": ["brief", "detailed"],
                    "default": "Detailed"
                },
                {
                    "id": "apply",
                    "question": "Apply changes?",
                    "type": "confirm",
                    "default": true
                }
            ]
        }))
        .unwrap();

        assert_eq!(request.questions[0].kind, QuestionnaireQuestionKind::Choice);
        assert_eq!(request.questions[0].default, Some(json!("detailed")));
        assert_eq!(
            request.questions[1].kind,
            QuestionnaireQuestionKind::Confirm
        );
        assert_eq!(request.questions[1].default, Some(json!("yes")));
    }

    #[test]
    fn questionnaire_question_serializes_type_field() {
        let question = QuestionnaireQuestion {
            id: "apply".into(),
            question: "Apply changes?".into(),
            help: None,
            default: Some(json!("no")),
            kind: QuestionnaireQuestionKind::Confirm,
            required: true,
            choices: Vec::new(),
            allow_other: false,
        };

        let value = serde_json::to_value(&question).unwrap();

        assert_eq!(value.get("type"), Some(&json!("confirm")));
        assert!(value.get("kind").is_none());
        let round_tripped: QuestionnaireQuestion = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped.kind, QuestionnaireQuestionKind::Confirm);
    }

    #[test]
    fn parse_request_accepts_kind_alias() {
        let request = parse_request(json!({
            "questions": [
                {
                    "id": "apply",
                    "question": "Apply changes?",
                    "kind": "confirm"
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            request.questions[0].kind,
            QuestionnaireQuestionKind::Confirm
        );
    }
}
