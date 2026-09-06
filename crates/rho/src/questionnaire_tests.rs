use super::*;

// Covers: defaults or malformed fallbacks must not become automatic answers.
// Owner: questionnaire argument parser.
#[test]
fn timeout_requires_explicit_complete_typed_fallbacks() {
    let valid = json!({
        "questions": [
            {"id":"color", "question":"Color?", "type":"choice", "choices":["red","blue"], "default":"red"},
            {"id":"checks", "question":"Checks?", "type":"multi_select", "choices":["unit","pty"]},
            {"id":"extra", "question":"Extra?", "type":"confirm", "required":false}
        ],
        "on_timeout":{"answers":{"color":"blue", "checks":["pty"]}, "reason":"Use blue and run PTY checks"}
    });
    let parsed = parse_request(valid.clone()).unwrap();
    pretty_assertions::assert_eq!(parsed.questions[0].default, Some(json!("red")));
    pretty_assertions::assert_eq!(
        parsed.on_timeout.unwrap().answers["color"],
        timeout::TimeoutAnswer::Text("blue".into())
    );

    for (pointer, value) in [
        ("/questions/0/id", Value::Null),
        ("/questions/0/id", json!("")),
        ("/questions/1/id", json!("color")),
        ("/on_timeout/reason", json!(" ")),
        ("/on_timeout/answers", json!({"checks":["pty"]})),
        (
            "/on_timeout/answers",
            json!({"color":"blue", "checks":["pty"], "unknown":true}),
        ),
        ("/on_timeout/answers/color", json!("green")),
        ("/on_timeout/answers/color", json!(true)),
        ("/on_timeout/answers/color", json!(["blue"])),
        ("/on_timeout/answers/color", Value::Null),
        ("/on_timeout/answers/color", json!(42)),
        ("/on_timeout/answers/color", json!({"value":"blue"})),
        ("/on_timeout/answers/checks", json!("pty")),
        ("/on_timeout/answers/checks", json!([])),
        ("/on_timeout/answers/checks", json!(["pty", "pty"])),
        ("/on_timeout/answers/checks", json!([1])),
        (
            "/on_timeout/answers",
            json!({"color":"blue", "checks":["pty"], "extra":"yes"}),
        ),
    ] {
        let mut input = valid.clone();
        *input.pointer_mut(pointer).unwrap() = value;
        assert!(parse_request(input.clone()).is_err(), "accepted {input}");
    }
    let mut input = valid;
    input["on_timeout"]["answers"]["extra"] = json!(false);
    let fallback = parse_request(input.clone()).unwrap().on_timeout.unwrap();
    pretty_assertions::assert_eq!(
        serde_json::to_value(&fallback).unwrap(),
        input["on_timeout"]
    );
    pretty_assertions::assert_eq!(
        fallback.host_response(),
        rho_sdk::HostInputResponse::new()
            .answer("color", ["blue"])
            .answer("checks", ["pty"])
            .answer("extra", ["no"])
    );
    input["on_timeout"]["timeout_seconds"] = json!(1);
    assert!(parse_request(input).is_err());
}

#[test]
fn parse_request_trims_optional_fields() {
    let request = parse_request(json!({
        "title": "  Edit target  ",
        "reason": "  I need a target  ",
        "questions": [
            {
                "id": " file ",
                "question": "  Which file?  ",
                "header": "  File  ",
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
            on_timeout: None,
            title: Some("Edit target".into()),
            reason: Some("I need a target".into()),
            questions: vec![QuestionnaireQuestion {
                id: "file".into(),
                question: "Which file?".into(),
                header: Some("File".into()),
                help: Some("Use a repo-relative path".into()),
                default: Some(json!("src/main.rs")),
                default_selection: QuestionnaireDefaultSelection::Selected,
                kind: QuestionnaireQuestionKind::Choice,
                required: true,
                choices: vec!["src/main.rs".into(), "src/lib.rs".into()],
                allow_other: false,
            }],
        }
    );
}

#[test]
fn parse_request_accepts_choice_descriptions() {
    let request = parse_request(json!({
        "questions": [{
            "id": "mode",
            "question": "Which mode?",
            "type": "choice",
            "choices": [
                { "label": "Fast", "description": "Finish sooner with fewer checks" },
                "Safe"
            ],
            "default": "fast"
        }]
    }))
    .unwrap();

    assert_eq!(
        request.questions[0].choices,
        vec![
            QuestionnaireChoice {
                label: "Fast".into(),
                description: Some("Finish sooner with fewer checks".into()),
            },
            QuestionnaireChoice::from("Safe"),
        ]
    );
    assert_eq!(request.questions[0].default, Some(json!("Fast")));
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
fn parse_request_accepts_focused_default_selection() {
    let request = parse_request(json!({
        "questions": [{
            "id": "prompt",
            "question": "Prompt mode?",
            "type": "choice",
            "choices": [
                { "label": "extend", "description": "Keep the standard prompt" },
                "replace"
            ],
            "default": "extend",
            "default_selection": "focused"
        }]
    }))
    .unwrap();

    assert_eq!(request.questions[0].default, Some(json!("extend")));
    assert_eq!(
        request.questions[0].default_selection,
        QuestionnaireDefaultSelection::Focused
    );
    assert_eq!(
        request.questions[0].choices,
        vec![
            QuestionnaireChoice {
                label: "extend".into(),
                description: Some("Keep the standard prompt".into()),
            },
            QuestionnaireChoice::from("replace"),
        ]
    );
}

#[test]
fn parse_request_rejects_focused_default_selection_without_default() {
    let err = parse_request(json!({
        "questions": [{
            "id": "prompt",
            "question": "Prompt mode?",
            "type": "choice",
            "choices": ["extend", "replace"],
            "default_selection": "focused"
        }]
    }))
    .unwrap_err();

    assert_eq!(
        err,
        "questions[0].default_selection focused requires default"
    );
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
