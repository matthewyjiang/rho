use agent_client_protocol::schema::v1::{
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, SessionId,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    HostChoice, HostInputRefusal, HostInputRequest, HostInputResponse, HostQuestion, SelectionMode,
    ToolCallId,
};
use serde_json::json;

use super::{elicitation_request, host_answer};

/// One question of each form control.
fn request() -> HostInputRequest {
    let questions = vec![
        HostQuestion::new(
            "scope",
            "Which scope should the change cover?",
            vec![
                HostChoice::new("crate", "One crate").description("Only the touched crate"),
                HostChoice::new("workspace", "Workspace"),
            ],
            SelectionMode::One,
        )
        .unwrap()
        .header("Scope")
        .help("Pick the narrowest that works.")
        .default_value(json!("workspace")),
        HostQuestion::new(
            "checks",
            "Which checks?",
            vec![
                HostChoice::new("fmt", "Format"),
                HostChoice::new("test", "Test"),
            ],
            SelectionMode::Many,
        )
        .unwrap()
        .optional(),
        HostQuestion::new(
            "notes",
            "Anything else?",
            vec![HostChoice::new("other", "Other")],
            SelectionMode::One,
        )
        .unwrap()
        .allow_other()
        .optional(),
        HostQuestion::new(
            "ship",
            "Open a PR?",
            vec![HostChoice::new("yes", "Yes"), HostChoice::new("no", "No")],
            SelectionMode::One,
        )
        .unwrap(),
        HostQuestion::new(
            "name",
            "Branch name?",
            vec![HostChoice::new("main", "main")],
            SelectionMode::One,
        )
        .unwrap()
        .allow_other(),
    ];
    HostInputRequest::questionnaire("Before I start", questions).unwrap()
}

// Covers: the form must carry every question in a shape both T3 Code (reads
// `enum`, `title`, `description`) and Zed (prefers titled `oneOf`) can draw,
// scoped to the asking tool call, with selected defaults
// Owner: ACP elicitation mapping
#[test]
fn questions_become_one_form_scoped_to_the_tool_call() {
    let form = elicitation_request(
        &request(),
        &SessionId::new("session-1"),
        Some(&ToolCallId::from_string("call-1").unwrap()),
    );

    assert_eq!(
        serde_json::to_value(form).unwrap(),
        json!({
            "mode": "form",
            "sessionId": "session-1",
            "toolCallId": "call-1",
            "message": "Before I start",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "checks": {
                        "type": "array",
                        "title": "Which checks?",
                        "items": {"anyOf": [
                            {"const": "fmt", "title": "Format"},
                            {"const": "test", "title": "Test"},
                        ]},
                    },
                    "name": {
                        "type": "string",
                        "title": "Branch name?",
                        "enum": ["main"],
                        "oneOf": [{"const": "main", "title": "main"}],
                    },
                    "name.other": {
                        "type": "string",
                        "title": "Branch name? (other)",
                        "description": "Your own answer instead of a listed choice",
                    },
                    "notes": {"type": "string", "title": "Anything else?"},
                    "scope": {
                        "type": "string",
                        "title": "Scope",
                        "description": "Which scope should the change cover?\n\nPick the narrowest that works.",
                        "default": "workspace",
                        "enum": ["crate", "workspace"],
                        "oneOf": [
                            {"const": "crate", "title": "One crate", "description": "Only the touched crate"},
                            {"const": "workspace", "title": "Workspace"},
                        ],
                    },
                    "ship": {"type": "boolean", "title": "Open a PR?"},
                },
                "required": ["scope", "ship"],
            },
        })
    );
}

fn accept(content: serde_json::Value) -> CreateElicitationResponse {
    let content = serde_json::from_value(content).unwrap();
    CreateElicitationResponse::new(ElicitationAction::Accept(
        ElicitationAcceptAction::new().content(Some(content)),
    ))
}

// Covers: accepted content becomes answers in each control's encoding (T3
// Code answers a checkbox with its text and every field with strings);
// decline and cancel are refusals; content that does not answer the request
// is treated as declined rather than handed to the tool
// Owner: ACP elicitation mapping
#[test]
fn host_responses_become_answers_or_refusals() {
    let answered = HostInputResponse::new()
        .answer("scope", ["crate"])
        .answer("checks", ["fmt", "test"])
        .answer("notes", ["ship it"])
        .answer("ship", ["yes"])
        .answer("name", ["feat/forms"]);
    let cases = [
        (
            "every control",
            accept(json!({
                "scope": "crate",
                "checks": ["fmt", "test"],
                "notes": "ship it",
                "ship": true,
                "name.other": "feat/forms",
            })),
            Ok(answered),
        ),
        (
            "text answers and blanks",
            accept(json!({
                "scope": "workspace",
                "notes": "  ",
                "ship": "false",
                "name": "main",
                "name.other": "",
            })),
            Ok(HostInputResponse::new()
                .answer("scope", ["workspace"])
                .answer("ship", ["no"])
                .answer("name", ["main"])),
        ),
        (
            "missing required answer",
            accept(json!({"scope": "crate", "ship": true})),
            Err(HostInputRefusal::Declined),
        ),
        (
            "unknown choice",
            accept(json!({"scope": "galaxy", "ship": true, "name": "main"})),
            Err(HostInputRefusal::Declined),
        ),
        (
            "decline",
            CreateElicitationResponse::new(ElicitationAction::Decline),
            Err(HostInputRefusal::Declined),
        ),
        (
            "cancel",
            CreateElicitationResponse::new(ElicitationAction::Cancel),
            Err(HostInputRefusal::Dismissed),
        ),
    ];
    for (label, response, expected) in cases {
        assert_eq!(host_answer(&request(), response), expected, "{label}");
    }
}
