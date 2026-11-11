use std::collections::BTreeMap;

use agent_client_protocol::v1::schema::{
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, SessionId,
};
use pretty_assertions::assert_eq;
use rho_protocol::{
    Choice, DefaultAnswer, DefaultStyle, InputAnswer, Question, QuestionAsk, Selection, ToolCallId,
};
use serde_json::json;

use super::{elicitation_request, host_answer as wire_answer, FormDialect};

/// A required question that takes only its choices, as the daemon sends it
/// before the builders below widen it.
fn question(id: &str, prompt: &str, choices: Vec<Choice>, selection: Selection) -> Question {
    Question {
        id: id.to_owned(),
        prompt: prompt.to_owned(),
        header: None,
        choices,
        selection,
        allow_other: false,
        help: None,
        default: None,
        required: true,
    }
}

fn choice(value: impl Into<String>, label: impl Into<String>) -> Choice {
    Choice {
        value: value.into(),
        label: label.into(),
        description: None,
    }
}

fn ask(title: &str, questions: Vec<Question>) -> QuestionAsk {
    QuestionAsk {
        title: title.to_owned(),
        questions,
    }
}

trait QuestionBuilder {
    fn with_header(self, header: &str) -> Self;
    fn with_help(self, help: &str) -> Self;
    /// Pre-selects `value`.
    fn with_default_value(self, value: &str) -> Self;
    fn optional(self) -> Self;
    fn allow_other(self) -> Self;
}

impl QuestionBuilder for Question {
    fn with_header(mut self, header: &str) -> Self {
        self.header = Some(header.to_owned());
        self
    }

    fn with_help(mut self, help: &str) -> Self {
        self.help = Some(help.to_owned());
        self
    }

    fn with_default_value(mut self, value: &str) -> Self {
        self.default = Some(DefaultAnswer {
            values: vec![value.to_owned()],
            style: DefaultStyle::Selected,
        });
        self
    }

    fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    fn allow_other(mut self) -> Self {
        self.allow_other = true;
        self
    }
}

fn with_description(mut choice: Choice, description: &str) -> Choice {
    choice.description = Some(description.to_owned());
    choice
}

/// The answers a form accepted with, built up one question at a time.
#[derive(Clone, Debug, Default, PartialEq)]
struct Answered(BTreeMap<String, Vec<String>>);

impl Answered {
    fn new() -> Self {
        Self::default()
    }

    fn answer<V: Into<String>>(mut self, id: &str, values: impl IntoIterator<Item = V>) -> Self {
        self.0
            .insert(id.to_owned(), values.into_iter().map(Into::into).collect());
        self
    }
}

#[derive(Debug, PartialEq)]
enum Refusal {
    Declined,
    Dismissed,
}

/// [`super::host_answer`], with its answers apart from its refusals.
fn host_answer(
    ask: &QuestionAsk,
    response: CreateElicitationResponse,
    dialect: FormDialect,
) -> Result<Answered, Refusal> {
    match wire_answer(ask, response, dialect) {
        InputAnswer::Answers { answers } => Ok(Answered(answers)),
        InputAnswer::Decline => Err(Refusal::Declined),
        InputAnswer::Dismiss => Err(Refusal::Dismissed),
        other => panic!("a form never answers {other:?}"),
    }
}

/// One question of each form control.
fn request() -> QuestionAsk {
    let questions = vec![
        question(
            "scope",
            "Which scope should the change cover?",
            vec![
                with_description(choice("crate", "One crate"), "Only the touched crate"),
                choice("workspace", "Workspace"),
            ],
            Selection::One,
        )
        .with_header("Scope")
        .with_help("Pick the narrowest that works.")
        .with_default_value("workspace"),
        question(
            "checks",
            "Which checks?",
            vec![choice("fmt", "Format"), choice("test", "Test")],
            Selection::Many,
        )
        .optional(),
        question(
            "notes",
            "Anything else?",
            vec![choice("other", "Other")],
            Selection::One,
        )
        .allow_other()
        .optional(),
        question(
            "ship",
            "Open a PR?",
            vec![choice("yes", "Yes"), choice("no", "No")],
            Selection::One,
        ),
        question(
            "name",
            "Branch name?",
            vec![choice("main", "main")],
            Selection::One,
        )
        .allow_other(),
    ];
    ask("Before I start", questions)
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
        Some(&ToolCallId::new("call-1")),
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
// decline and cancel are refusals. The daemon checks the answers.
// Owner: ACP elicitation mapping
#[test]
fn host_responses_become_answers_or_refusals() {
    let answered = Answered::new()
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
            Ok(Answered::new()
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
            Ok(Answered::new()
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
            "decline",
            FormDialect::Standard,
            CreateElicitationResponse::new(ElicitationAction::Decline),
            Err(Refusal::Declined),
        ),
        (
            "cancel",
            FormDialect::T3Code,
            CreateElicitationResponse::new(ElicitationAction::Cancel),
            Err(Refusal::Dismissed),
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
    let request = ask(
        "Server form",
        vec![
            question("x", "X?", vec![choice("a", "A")], Selection::One).allow_other(),
            question(
                "x.other",
                "Note?",
                vec![choice("other", "Other")],
                Selection::One,
            )
            .allow_other(),
        ],
    );
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
        Ok(Answered::new()
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
        question(
            "q",
            "Pick",
            pairs
                .iter()
                .map(|(value, label)| choice(*value, *label))
                .collect(),
            selection,
        )
    };
    let cases = [
        (
            "exact label over a case-folded one",
            choices(Selection::One, &[("lower", "foo"), ("upper", "FOO")]),
            "FOO",
            vec!["upper"],
        ),
        (
            "exact label over another choice's value",
            choices(Selection::One, &[("X", "Y"), ("Z", "X")]),
            "X",
            vec!["Z"],
        ),
        (
            "a comma label switches the list to semicolons",
            choices(
                Selection::Many,
                &[("a", "A"), ("b", "B"), ("c", "C"), ("ab", "A, B")],
            ),
            "A; B; C",
            vec!["a", "b", "c"],
        ),
        (
            "labels with every common separator and a new line",
            choices(
                Selection::Many,
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
                Selection::Many,
                &[("a", "A"), ("b", "B"), ("c", "C"), ("ab", "A, B")],
            ),
            "A, B; C",
            vec!["ab", "c"],
        ),
    ];
    for (label, question, typed, expected) in cases {
        let request = ask("Pick", vec![question]);
        assert_eq!(
            host_answer(&request, accept(json!({"q": typed})), FormDialect::T3Code),
            Ok(Answered::new().answer("q", expected)),
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
        let question = question(
            "q",
            "Pick",
            labels
                .iter()
                .enumerate()
                .map(|(index, label)| choice(format!("v{index}"), *label))
                .collect(),
            Selection::Many,
        );
        let request = ask("Pick", vec![question]);
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
    let request = ask(
        "Pick",
        vec![question(
            "q",
            "Pick",
            vec![choice("all", every.as_str()), choice("b", "B")],
            Selection::Many,
        )],
    );
    assert_eq!(
        host_answer(&request, accept(json!({"q": every})), FormDialect::T3Code),
        Ok(Answered::new().answer("q", ["all"]))
    );
}

// Covers: a default names its choice by value even when that value is
// another choice's label; a default given as a label still selects it
// Owner: ACP elicitation mapping
#[test]
fn defaults_prefer_a_choice_value_over_another_label() {
    let default_for = |default: &str| {
        let question = question(
            "q",
            "Pick",
            vec![choice("first", "second"), choice("second", "Other")],
            Selection::One,
        )
        .with_default_value(default);
        let request = ask("Pick", vec![question]);
        let form = elicitation_request(&request, &SessionId::new("s"), None, FormDialect::Standard);
        serde_json::to_value(form).unwrap()["requestedSchema"]["properties"]["q"]["default"].clone()
    };
    assert_eq!(
        [default_for("second"), default_for("other")],
        [json!("second"), json!("second")]
    );
}
