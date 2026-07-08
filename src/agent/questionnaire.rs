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
    pub default: Option<String>,
    #[serde(default)]
    pub kind: QuestionnaireQuestionKind,
    #[serde(default = "default_required")]
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionnaireQuestionKind {
    #[default]
    Text,
    Choice,
    Confirm,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireResponse {
    pub answers: Vec<QuestionnaireAnswer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireAnswer {
    pub id: String,
    pub answer: String,
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
    #[serde(rename = "type", default)]
    kind: Option<QuestionnaireQuestionKind>,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    choices: Vec<String>,
}

pub fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: TOOL_NAME.into(),
        description: "Ask a concise user form only when missing input blocks correctness, safety, or requested preferences. Main mode: group all related missing inputs into questions[]. Prefer assumptions when safe; ask once, not a chain of single questions.".into(),
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
                        "required": ["question"],
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
                                "enum": ["text", "choice", "confirm"],
                                "description": "Question type. Use choice when valid answers are known, confirm for yes/no. Defaults to text."
                            },
                            "choices": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Allowed answers for choice questions."
                            },
                            "default": {
                                "description": "Optional default answer. For confirm, use true/false or yes/no.",
                                "oneOf": [
                                    { "type": "string" },
                                    { "type": "boolean" },
                                    { "type": "number" }
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
        .map(|(index, question)| parse_question(index, question))
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
            lines.push(format!("   default: {default}"));
        }
        if !question.choices.is_empty() {
            lines.push(format!("   choices: {}", question.choices.join(", ")));
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

fn parse_question(
    index: usize,
    raw: RawQuestionnaireQuestion,
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
    if matches!(kind, QuestionnaireQuestionKind::Choice) && choices.is_empty() {
        return Err(format!(
            "questions[{index}].choices must include at least one choice"
        ));
    }
    if !matches!(kind, QuestionnaireQuestionKind::Choice) && !choices.is_empty() {
        return Err(format!(
            "questions[{index}].choices is only valid for choice questions"
        ));
    }

    let default = normalize_default(index, raw.default, kind, &choices)?;
    Ok(QuestionnaireQuestion {
        id,
        question,
        help: trim_optional(raw.help).or_else(|| trim_optional(raw.reason)),
        default,
        kind,
        required: raw.required.unwrap_or_else(default_required),
        choices,
    })
}

fn normalize_default(
    index: usize,
    default: Option<Value>,
    kind: QuestionnaireQuestionKind,
    choices: &[String],
) -> Result<Option<String>, String> {
    let Some(default) = default else {
        return Ok(None);
    };
    let value = match default {
        Value::Null => return Ok(None),
        Value::String(value) => value,
        Value::Bool(value) if matches!(kind, QuestionnaireQuestionKind::Confirm) => {
            if value { "yes" } else { "no" }.into()
        }
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(_) | Value::Object(_) => {
            return Err(format!(
                "questions[{index}].default must be a string, boolean, or number"
            ));
        }
    };
    let Some(value) = trim_optional(Some(value)) else {
        return Ok(None);
    };
    match kind {
        QuestionnaireQuestionKind::Text => Ok(Some(value)),
        QuestionnaireQuestionKind::Choice => choices
            .iter()
            .find(|choice| choice.eq_ignore_ascii_case(&value))
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("questions[{index}].default must match one of the choices")),
        QuestionnaireQuestionKind::Confirm => normalize_confirm_value(&value)
            .map(Some)
            .ok_or_else(|| format!("questions[{index}].default must be yes or no")),
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
                    default: Some("src/main.rs".into()),
                    kind: QuestionnaireQuestionKind::Text,
                    required: true,
                    choices: Vec::new(),
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
        assert_eq!(request.questions[0].default.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn parse_request_rejects_empty_questions() {
        let err = parse_request(json!({ "questions": [] })).unwrap_err();

        assert_eq!(err, "questions must include at least one question");
    }

    #[test]
    fn parse_request_normalizes_choice_and_confirm_defaults() {
        let request = parse_request(json!({
            "questions": [
                {
                    "id": "style",
                    "question": "Style?",
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
        assert_eq!(request.questions[0].default.as_deref(), Some("detailed"));
        assert_eq!(
            request.questions[1].kind,
            QuestionnaireQuestionKind::Confirm
        );
        assert_eq!(request.questions[1].default.as_deref(), Some("yes"));
    }
}
