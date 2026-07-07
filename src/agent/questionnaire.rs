use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::ToolSpec;

pub const TOOL_NAME: &str = "questionnaire";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireRequest {
    pub question: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionnaireResponse {
    pub answer: String,
}

pub fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: TOOL_NAME.into(),
        description: "Ask the user one concise clarifying question and wait for their answer. Use this only when missing user input materially blocks correctness, safety, or a requested preference. Before asking, inspect available context and make reasonable assumptions when safe. Ask the fewest questions possible, include a reason when helpful, and provide a default when one answer is likely.".into(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["question"],
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The exact concise question to show the user."
                },
                "reason": {
                    "type": "string",
                    "description": "Optional brief explanation of why this answer is needed."
                },
                "default": {
                    "type": "string",
                    "description": "Optional default answer the user may accept by submitting an empty response."
                }
            }
        }),
    }
}

pub fn parse_request(arguments: Value) -> Result<QuestionnaireRequest, String> {
    let request: QuestionnaireRequest = serde_json::from_value(arguments)
        .map_err(|err| format!("invalid questionnaire arguments: {err}"))?;
    if request.question.trim().is_empty() {
        return Err("question must not be empty".into());
    }
    Ok(QuestionnaireRequest {
        question: request.question.trim().to_string(),
        reason: request
            .reason
            .map(|reason| reason.trim().to_string())
            .filter(|reason| !reason.is_empty()),
        default: request
            .default
            .map(|default| default.trim().to_string())
            .filter(|default| !default.is_empty()),
    })
}

pub fn response_content(response: &QuestionnaireResponse) -> String {
    serde_json::to_string(response).unwrap_or_else(|_| format!("answer: {}", response.answer))
}

pub fn start_display_lines(request: &QuestionnaireRequest) -> Vec<String> {
    let mut lines = vec![TOOL_NAME.to_string(), request.question.clone()];
    if let Some(reason) = &request.reason {
        lines.push(format!("reason: {reason}"));
    }
    if let Some(default) = &request.default {
        lines.push(format!("default: {default}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_request_trims_optional_fields() {
        let request = parse_request(json!({
            "question": "  Which file?  ",
            "reason": "  I need a target  ",
            "default": "  src/main.rs  "
        }))
        .unwrap();

        assert_eq!(
            request,
            QuestionnaireRequest {
                question: "Which file?".into(),
                reason: Some("I need a target".into()),
                default: Some("src/main.rs".into()),
            }
        );
    }

    #[test]
    fn parse_request_rejects_empty_question() {
        let err = parse_request(json!({ "question": "   " })).unwrap_err();

        assert_eq!(err, "question must not be empty");
    }
}
