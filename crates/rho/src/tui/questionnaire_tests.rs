use rho_sdk::{HostChoice, HostInputRequest, HostQuestion, SelectionMode};

use super::*;

fn choice(label: &str) -> HostChoice {
    HostChoice::new(label, label)
}

fn yes_no() -> Vec<HostChoice> {
    vec![HostChoice::new("yes", "yes"), HostChoice::new("no", "no")]
}

fn other_question() -> HostQuestion {
    HostQuestion::new("file", "Which file?", vec![choice("a")], SelectionMode::One)
        .unwrap()
        .allow_other()
}

fn host_request(title: &str, questions: Vec<HostQuestion>) -> HostInputRequest {
    HostInputRequest::questionnaire(title, questions).unwrap()
}

#[test]
fn cancel_sends_user_cancelled_reply() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer = QuestionnaireComposer::new(
        host_request("", vec![other_question()]),
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
        host_request(
            "PR details",
            vec![
                HostQuestion::new(
                    "branch",
                    "Which branch?",
                    vec![choice("main"), choice("develop")],
                    SelectionMode::One,
                )
                .unwrap()
                .default_value(serde_json::json!("main"))
                .allow_other(),
                HostQuestion::new(
                    "test_suites",
                    "Which test suites should I run?",
                    vec![choice("unit"), choice("e2e"), choice("lint")],
                    SelectionMode::Many,
                )
                .unwrap()
                .default_value(serde_json::json!(["unit"])),
                HostQuestion::new("apply", "Apply changes?", yes_no(), SelectionMode::One)
                    .unwrap()
                    .default_value(serde_json::json!("yes")),
            ],
        ),
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

    assert!(!submitted.display.is_empty());
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
    let question =
        HostQuestion::new("apply", "Apply changes?", yes_no(), SelectionMode::One).unwrap();
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
    let question = HostQuestion::new(
        "targets",
        "Targets?",
        vec![choice("New York, NY"), choice("Boston, MA")],
        SelectionMode::Many,
    )
    .unwrap()
    .default_value(serde_json::json!(["New York, NY", "Los Angeles, CA"]))
    .allow_other();

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

fn form_composer() -> QuestionnaireComposer {
    QuestionnaireComposer::new(
        host_request(
            "PR details",
            vec![
                HostQuestion::new(
                    "branch",
                    "Which branch?",
                    vec![choice("main"), choice("develop")],
                    SelectionMode::One,
                )
                .unwrap()
                .default_value(serde_json::json!("main"))
                .allow_other(),
                HostQuestion::new(
                    "suites",
                    "Which suites?",
                    vec![choice("unit"), choice("e2e")],
                    SelectionMode::Many,
                )
                .unwrap(),
            ],
        ),
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    )
}

#[test]
fn failed_submit_jumps_to_the_offending_question() {
    let mut composer = form_composer();
    // Question 1 has a default answer; question 2 (multi_select, required)
    // is unanswered. Submit from the last question.
    composer.active_index = 1;

    let error = composer.submit().unwrap_err();

    assert!(error.starts_with("question 2:"), "{error}");
    assert_eq!(composer.active_index, 1);

    // Clear question 1's answer as well: the jump targets the first failure.
    composer.active_index = 0;
    composer.clear_active_answer();
    composer.active_index = 1;
    let error = composer.submit().unwrap_err();

    assert!(error.starts_with("question 1:"), "{error}");
    assert_eq!(composer.active_index, 0);
}

#[test]
fn arrow_navigation_flows_across_questions() {
    let mut composer = form_composer();
    assert_eq!(composer.active_index, 0);

    composer.move_down(); // main -> develop
    composer.move_down(); // develop -> other
    assert_eq!(composer.active_index, 0);
    composer.move_down(); // other row is last -> next question
    assert_eq!(composer.active_index, 1);

    composer.move_up(); // first choice of q2 -> previous question
    assert_eq!(composer.active_index, 0);
}

#[test]
fn word_navigation_and_deletion_stay_with_composer_state() {
    let mut composer = QuestionnaireComposer::new(
        host_request("", vec![other_question()]),
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    );
    composer.insert_text("alpha beta");

    composer.move_text_cursor_previous_word();
    assert_eq!(composer.active_field().other_cursor, 6);
    composer.delete_previous_word();
    assert_eq!(composer.active_field().other_value, "beta");
}
