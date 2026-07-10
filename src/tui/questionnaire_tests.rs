use super::*;

fn text_question() -> QuestionnaireQuestion {
    QuestionnaireQuestion {
        id: "file".into(),
        question: "Which file?".into(),
        help: None,
        default: None,
        kind: QuestionnaireQuestionKind::Text,
        required: true,
        choices: Vec::new(),
        allow_other: false,
    }
}

#[test]
fn cancel_sends_user_cancelled_reply() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![text_question()],
        },
        QuestionnaireResponseChannel::new(reply_tx),
    );

    composer.cancel_by_user();

    assert!(matches!(
        reply_rx.try_recv(),
        Ok(QuestionnaireReply::Cancelled(
            QuestionnaireCancelReason::UserCancelled
        ))
    ));
}

#[test]
fn submit_sends_selection_answers() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: Some("PR details".into()),
            reason: Some("Need missing preferences".into()),
            questions: vec![
                QuestionnaireQuestion {
                    id: "branch".into(),
                    question: "Which branch?".into(),
                    help: None,
                    default: Some(serde_json::json!("main")),
                    kind: QuestionnaireQuestionKind::Choice,
                    required: true,
                    choices: vec!["main".into(), "develop".into()],
                    allow_other: true,
                },
                QuestionnaireQuestion {
                    id: "test_suites".into(),
                    question: "Which test suites should I run?".into(),
                    help: None,
                    default: Some(serde_json::json!(["unit"])),
                    kind: QuestionnaireQuestionKind::MultiSelect,
                    required: true,
                    choices: vec!["unit".into(), "e2e".into(), "lint".into()],
                    allow_other: false,
                },
                QuestionnaireQuestion {
                    id: "apply".into(),
                    question: "Apply changes?".into(),
                    help: None,
                    default: Some(serde_json::json!("yes")),
                    kind: QuestionnaireQuestionKind::Confirm,
                    required: true,
                    choices: Vec::new(),
                    allow_other: false,
                },
            ],
        },
        QuestionnaireResponseChannel::new(reply_tx),
    );
    composer.fields[0].selection = FieldSelection::Other;
    composer.fields[0].choice_cursor = 2;
    composer.fields[0].other_value = "release".into();
    composer.fields[0].other_cursor = "release".chars().count();
    composer.fields[1].selection = FieldSelection::Multi {
        selected: vec![0, 1],
        other: false,
    };
    composer.fields[2].selection = FieldSelection::Single(1);

    let submitted = composer.submit().unwrap();

    assert!(
        submitted.display.contains("Which branch?: release"),
        "{}",
        submitted.display
    );
    assert!(
        submitted
            .display
            .contains("Which test suites should I run?: unit, e2e"),
        "{}",
        submitted.display
    );
    assert!(
        submitted.display.contains("Apply changes?: no"),
        "{}",
        submitted.display
    );
    assert!(matches!(
        reply_rx.try_recv(),
        Ok(QuestionnaireReply::Answer(QuestionnaireResponse { answers }))
            if answers == vec![
                QuestionnaireAnswer { id: "branch".into(), answer: serde_json::json!("release") },
                QuestionnaireAnswer { id: "test_suites".into(), answer: serde_json::json!(["unit", "e2e"]) },
                QuestionnaireAnswer { id: "apply".into(), answer: serde_json::json!("no") },
            ]
    ));
}

#[test]
fn required_confirm_without_default_requires_explicit_choice() {
    let question = QuestionnaireQuestion {
        id: "apply".into(),
        question: "Apply changes?".into(),
        help: None,
        default: None,
        kind: QuestionnaireQuestionKind::Confirm,
        required: true,
        choices: Vec::new(),
        allow_other: false,
    };
    let field = QuestionnaireFieldState::new(&question);

    assert_eq!(field.selection, FieldSelection::None);
    assert_eq!(
        normalize_questionnaire_answer(&question, &field),
        Err("answer is not selected".into())
    );

    let mut field = field;
    field.toggle_highlighted(&question);
    assert_eq!(
        normalize_questionnaire_answer(&question, &field),
        Ok(serde_json::json!("yes"))
    );
}

#[test]
fn multi_select_default_preserves_commas() {
    let question = QuestionnaireQuestion {
        id: "targets".into(),
        question: "Targets?".into(),
        help: None,
        default: Some(serde_json::json!(["New York, NY", "Los Angeles, CA"])),
        kind: QuestionnaireQuestionKind::MultiSelect,
        required: true,
        choices: vec!["New York, NY".into(), "Boston, MA".into()],
        allow_other: true,
    };

    let field = QuestionnaireFieldState::new(&question);

    assert_eq!(
        field.selection,
        FieldSelection::Multi {
            selected: vec![0],
            other: true
        }
    );
    assert_eq!(field.other_value, "Los Angeles, CA");
    assert_eq!(
        normalize_questionnaire_answer(&question, &field),
        Ok(serde_json::json!(["New York, NY", "Los Angeles, CA"]))
    );
}

#[test]
fn cursor_counts_whitespace_wrapped_question_lines() {
    let question = QuestionnaireQuestion {
        id: "style".into(),
        question: "hello wide world".into(),
        help: None,
        default: None,
        kind: QuestionnaireQuestionKind::Choice,
        required: true,
        choices: vec!["brief".into(), "detailed".into()],
        allow_other: false,
    };
    let field = QuestionnaireFieldState::new(&question);
    let expected_cursor = questionnaire_question_cursor(&question, &field, 3, 14);
    let rendered = questionnaire_frame(
        &QuestionnaireComposer {
            request: QuestionnaireRequest {
                title: None,
                reason: None,
                questions: vec![question.clone()],
            },
            response: QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
            fields: vec![field],
            active_index: 0,
        },
        14,
    );

    assert_eq!(expected_cursor, rendered.1);
    assert_eq!(expected_cursor.y, 6);
}

#[test]
fn word_navigation_and_deletion_stay_with_composer_state() {
    let mut composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![text_question()],
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    );
    composer.insert_text("alpha beta");

    composer.move_text_cursor_previous_word();
    assert_eq!(composer.active_field().other_cursor, 6);
    composer.delete_previous_word();
    assert_eq!(composer.active_field().other_value, "beta");
}
