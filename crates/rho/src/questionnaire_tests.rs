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
fn parse_request_trims_choice_descriptions() {
    let request = parse_request(json!({
        "questions": [{
            "question": "Which mode?",
            "type": "choice",
            "choices": [{ "label": " Fast ", "description": "  Quicker  " }]
        }]
    }))
    .unwrap();

    assert_eq!(request.questions[0].choices[0].label, "Fast");
    assert_eq!(
        request.questions[0].choices[0].description.as_deref(),
        Some("Quicker")
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
        header: None,
        help: None,
        default: Some(json!("no")),
        default_selection: QuestionnaireDefaultSelection::Selected,
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
fn questionnaire_question_deserializes_legacy_and_detailed_choices() {
    let question: QuestionnaireQuestion = serde_json::from_value(json!({
        "id": "mode",
        "question": "Which mode?",
        "type": "choice",
        "choices": [
            "Fast",
            { "label": "Safe", "description": "Run every check" }
        ]
    }))
    .unwrap();

    assert_eq!(
        question.choices,
        vec![
            QuestionnaireChoice::from("Fast"),
            QuestionnaireChoice {
                label: "Safe".into(),
                description: Some("Run every check".into()),
            },
        ]
    );
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
