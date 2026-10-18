use agent_client_protocol::schema::v1::{
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, SessionId,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    HostChoice, HostInputRefusal, HostInputRequest, HostInputResponse, HostQuestion, SelectionMode,
    ToolCallId,
};
use serde_json::json;

use super::{elicitation_request, host_answer, FormDialect};

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

// Covers: the standard form carries every question in the ACP schema (titled
// `oneOf`, string arrays, a free-text companion), scoped to the asking tool
// call, with selected defaults
// Owner: ACP elicitation mapping
#[test]
fn questions_become_one_form_scoped_to_the_tool_call() {
    let form = elicitation_request(
        &request(),
        &SessionId::new("session-1"),
        Some(&ToolCallId::from_string("call-1").unwrap()),
        FormDialect::Standard,
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

// Covers: T3 Code reads only `enum`, `title`, and `description` and lets any
// question take free text, so each question is one string property listing
// choice labels, with no free-text companion
// Owner: ACP elicitation mapping
#[test]
fn t3_code_gets_one_labelled_string_per_question() {
    let form = elicitation_request(
        &request(),
        &SessionId::new("session-1"),
        None,
        FormDialect::T3Code,
    );

    assert_eq!(
        serde_json::to_value(form).unwrap()["requestedSchema"],
        json!({
            "type": "object",
            "properties": {
                "checks": {
                    "type": "string",
                    "title": "Which checks?",
                    "description": "Pick one or more; separate several with commas.",
                    "enum": ["Format", "Test"],
                },
                "name": {"type": "string", "title": "Branch name?", "enum": ["main"]},
                "notes": {"type": "string", "title": "Anything else?"},
                "scope": {
                    "type": "string",
                    "title": "Scope",
                    "description": "Which scope should the change cover?\n\nPick the narrowest that works.",
                    "enum": ["One crate", "Workspace"],
                },
                "ship": {"type": "boolean", "title": "Open a PR?"},
            },
            "required": ["scope", "ship", "name"],
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
            FormDialect::Standard,
            accept(json!({
                "scope": "crate",
                "checks": ["fmt", "test"],
                "notes": "ship it",
                "ship": true,
                "name.other": "feat/forms",
            })),
            Ok(answered.clone()),
        ),
        (
            "text answers and blanks",
            FormDialect::Standard,
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
            "a typed answer replaces a selection the host cannot clear",
            FormDialect::Standard,
            accept(
                json!({"scope": "crate", "ship": true, "name": "main", "name.other": "feat/forms"}),
            ),
            Ok(HostInputResponse::new()
                .answer("scope", ["crate"])
                .answer("ship", ["yes"])
                .answer("name", ["feat/forms"])),
        ),
        (
            "t3 code answers with labels, comma lists, and free text",
            FormDialect::T3Code,
            accept(json!({
                "scope": "One crate",
                "checks": "Format, test",
                "notes": "ship it",
                "ship": "true",
                "name": "feat/forms",
            })),
            Ok(answered),
        ),
        (
            "t3 code free text for a closed choice",
            FormDialect::T3Code,
            accept(
                json!({"scope": "galaxy", "checks": "fmt", "notes": "x", "ship": "true", "name": "main"}),
            ),
            Err(HostInputRefusal::Declined),
        ),
        (
            "missing required answer",
            FormDialect::Standard,
            accept(json!({"scope": "crate", "ship": true})),
            Err(HostInputRefusal::Declined),
        ),
        (
            "unknown choice",
            FormDialect::Standard,
            accept(json!({"scope": "galaxy", "ship": true, "name": "main"})),
            Err(HostInputRefusal::Declined),
        ),
        (
            "decline",
            FormDialect::Standard,
            CreateElicitationResponse::new(ElicitationAction::Decline),
            Err(HostInputRefusal::Declined),
        ),
        (
            "cancel",
            FormDialect::T3Code,
            CreateElicitationResponse::new(ElicitationAction::Cancel),
            Err(HostInputRefusal::Dismissed),
        ),
    ];
    for (label, dialect, response, expected) in cases {
        assert_eq!(
            host_answer(&request(), response, dialect),
            expected,
            "{label}"
        );
    }
}

// Covers: MCP property names may contain `.`, so a free-text companion key
// must not collide with another question's property
// Owner: ACP elicitation mapping
#[test]
fn free_text_companions_do_not_collide_with_question_ids() {
    let request = HostInputRequest::questionnaire(
        "Server form",
        vec![
            HostQuestion::new(
                "x",
                "X?",
                vec![HostChoice::new("a", "A")],
                SelectionMode::One,
            )
            .unwrap()
            .allow_other(),
            HostQuestion::new(
                "x.other",
                "Note?",
                vec![HostChoice::new("other", "Other")],
                SelectionMode::One,
            )
            .unwrap()
            .allow_other(),
        ],
    )
    .unwrap();
    let form = elicitation_request(&request, &SessionId::new("s"), None, FormDialect::Standard);
    let schema = serde_json::to_value(form).unwrap()["requestedSchema"]["properties"].clone();
    let mut keys = schema
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();

    assert_eq!(keys, ["x", "x.other", "x.other_"]);
    assert_eq!(
        host_answer(
            &request,
            accept(json!({"x.other_": "mine", "x.other": "note"})),
            FormDialect::Standard,
        ),
        Ok(HostInputResponse::new()
            .answer("x", ["mine"])
            .answer("x.other", ["note"]))
    );
}

// Covers: T3 Code answers with displayed labels, several joined by a
// separator no label contains; an exact label wins over another choice's
// value or a case-folded label, and a list splits into exactly the choices
// listed even when a label contains commas
// Owner: ACP elicitation mapping
#[test]
fn t3_code_labels_resolve_to_their_own_choices() {
    let choices = |selection, pairs: &[(&str, &str)]| {
        HostQuestion::new(
            "q",
            "Pick",
            pairs
                .iter()
                .map(|(value, label)| HostChoice::new(*value, *label))
                .collect(),
            selection,
        )
        .unwrap()
    };
    let cases = [
        (
            "exact label over a case-folded one",
            choices(SelectionMode::One, &[("lower", "foo"), ("upper", "FOO")]),
            "FOO",
            vec!["upper"],
        ),
        (
            "exact label over another choice's value",
            choices(SelectionMode::One, &[("X", "Y"), ("Z", "X")]),
            "X",
            vec!["Z"],
        ),
        (
            "a comma label switches the list to semicolons",
            choices(
                SelectionMode::Many,
                &[("a", "A"), ("b", "B"), ("c", "C"), ("ab", "A, B")],
            ),
            "A; B; C",
            vec!["a", "b", "c"],
        ),
        (
            "labels with every common separator and a new line",
            choices(
                SelectionMode::Many,
                &[
                    ("a", "A"),
                    ("b", "B"),
                    ("ab", "A\nB"),
                    ("mix", "A, B; C | D / E + F"),
                ],
            ),
            "A\nB • A",
            vec!["ab", "a"],
        ),
        (
            "the comma label in a semicolon list",
            choices(
                SelectionMode::Many,
                &[("a", "A"), ("b", "B"), ("c", "C"), ("ab", "A, B")],
            ),
            "A, B; C",
            vec!["ab", "c"],
        ),
    ];
    for (label, question, typed, expected) in cases {
        let request = HostInputRequest::questionnaire("Pick", vec![question]).unwrap();
        assert_eq!(
            host_answer(&request, accept(json!({"q": typed})), FormDialect::T3Code),
            Ok(HostInputResponse::new().answer("q", expected)),
            "{label}"
        );
    }
}

// Covers: the form's hint names the separator a typed multiple choice must
// use, so the user can type a list that splits back unambiguously; labels
// that leave no separator get a single choice instead of a panic
// Owner: ACP elicitation mapping
#[test]
fn t3_code_multiple_choice_hint_names_a_separator_no_label_contains() {
    let hint = |labels: &[&str]| {
        let question = HostQuestion::new(
            "q",
            "Pick",
            labels
                .iter()
                .enumerate()
                .map(|(index, label)| HostChoice::new(format!("v{index}"), *label))
                .collect(),
            SelectionMode::Many,
        )
        .unwrap();
        let request = HostInputRequest::questionnaire("Pick", vec![question]).unwrap();
        let form = elicitation_request(&request, &SessionId::new("s"), None, FormDialect::T3Code);
        serde_json::to_value(form).unwrap()["requestedSchema"]["properties"]["q"]["description"]
            .clone()
    };
    for (labels, expected) in [
        (&["A", "B"][..], "commas"),
        (&["A, B", "C"][..], "semicolons"),
        (&["A, B", "C; D"][..], "\"|\""),
        (&["A, B", "C; D | E / F + G"][..], "\"•\""),
    ] {
        assert_eq!(
            hint(labels),
            json!(format!(
                "Pick one or more; separate several with {expected}."
            )),
            "{labels:?}"
        );
    }
    // A label using every candidate leaves no separator: one typed choice,
    // never a split, and no panic.
    let every = ",;|/+•"
        .chars()
        .chain('\u{2190}'..='\u{2BFF}')
        .collect::<String>();
    assert_eq!(hint(&[every.as_str(), "B"]), json!("Pick one."));
    let request = HostInputRequest::questionnaire(
        "Pick",
        vec![HostQuestion::new(
            "q",
            "Pick",
            vec![
                HostChoice::new("all", every.as_str()),
                HostChoice::new("b", "B"),
            ],
            SelectionMode::Many,
        )
        .unwrap()],
    )
    .unwrap();
    assert_eq!(
        host_answer(&request, accept(json!({"q": every})), FormDialect::T3Code),
        Ok(HostInputResponse::new().answer("q", ["all"]))
    );
}
