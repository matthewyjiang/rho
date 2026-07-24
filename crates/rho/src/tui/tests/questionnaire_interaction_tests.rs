use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    questionnaire::{
        QuestionnaireAnswer, QuestionnaireDefaultSelection, QuestionnaireQuestionKind,
        QuestionnaireResponse,
    },
    tui::questionnaire::{QuestionnaireQuestion, QuestionnaireRequest},
};

use super::*;

fn choice_question(id: &str) -> QuestionnaireQuestion {
    QuestionnaireQuestion {
        id: id.into(),
        question: format!("{id}?"),
        header: None,
        help: None,
        default: None,
        default_selection: QuestionnaireDefaultSelection::Selected,
        kind: QuestionnaireQuestionKind::Choice,
        required: true,
        choices: vec!["alpha".into(), "beta".into()],
        allow_other: false,
    }
}

fn confirm_question(id: &str) -> QuestionnaireQuestion {
    QuestionnaireQuestion {
        id: id.into(),
        question: format!("{id}?"),
        header: None,
        help: None,
        default: None,
        default_selection: QuestionnaireDefaultSelection::Selected,
        kind: QuestionnaireQuestionKind::Confirm,
        required: true,
        choices: Vec::new(),
        allow_other: false,
    }
}

#[test]
fn enter_advances_questions_and_submits_only_on_the_last() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut app = test_app();
    app.input_ui
        .set_composer(ComposerMode::Questionnaire(QuestionnaireComposer::new(
            QuestionnaireRequest {
                title: None,
                reason: None,
                questions: vec![choice_question("first"), confirm_question("second")],
            },
            QuestionnaireResponseChannel::new(reply_tx),
        )));
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

    assert!(app.handle_questionnaire_key(enter).unwrap());
    assert!(
        matches!(app.input_ui.composer(), ComposerMode::Questionnaire(_)),
        "enter on the first question must not submit the form"
    );
    assert!(reply_rx.try_recv().is_err());

    assert!(app.handle_questionnaire_key(enter).unwrap());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(
        reply_rx.try_recv(),
        Ok(QuestionnaireReply::Answer(QuestionnaireResponse {
            answers: vec![
                QuestionnaireAnswer {
                    id: "first".into(),
                    answer: serde_json::json!("alpha"),
                },
                QuestionnaireAnswer {
                    id: "second".into(),
                    answer: serde_json::json!("yes"),
                },
            ],
        }))
    );
}

#[test]
fn resolving_questionnaire_clears_preexisting_shell_mode() {
    for submit in [true, false] {
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        let mut app = test_app();
        app.input_ui
            .set_shell_mode(Some(InlineShellMode::ExcludeFromContext));
        app.input_ui
            .set_composer(ComposerMode::Questionnaire(QuestionnaireComposer::new(
                QuestionnaireRequest {
                    title: None,
                    reason: None,
                    questions: vec![choice_question("choice")],
                },
                QuestionnaireResponseChannel::new(reply_tx),
            )));

        let key = if submit {
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        } else {
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
        };
        app.handle_questionnaire_key(key).unwrap();

        assert_eq!(app.input_ui.shell_mode(), None);
        assert!(app.input_ui.text().is_empty());
        assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    }
}

#[test]
fn second_ctrl_c_cancels_questionnaire_without_exiting_tui() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut app = test_app();
    app.input_ui
        .set_composer(ComposerMode::Questionnaire(QuestionnaireComposer::new(
            QuestionnaireRequest {
                title: None,
                reason: None,
                questions: vec![QuestionnaireQuestion {
                    id: "answer".into(),
                    question: "Continue?".into(),
                    header: None,
                    help: None,
                    default: None,
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::Confirm,
                    required: true,
                    choices: Vec::new(),
                    allow_other: false,
                }],
            },
            QuestionnaireResponseChannel::new(reply_tx),
        )));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

    assert!(app.handle_questionnaire_key(ctrl_c).unwrap());
    assert_eq!(app.status, "answer cleared; press ctrl-c again to cancel");
    assert!(app.handle_questionnaire_key(ctrl_c).unwrap());

    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert!(!app.should_quit);
    assert_eq!(app.ctrl_c_streak, 0);
    assert_eq!(app.status, "answer cancelled");
    assert!(matches!(
        reply_rx.try_recv(),
        Ok(QuestionnaireReply::Cancelled(
            QuestionnaireCancelReason::UserCancelled
        ))
    ));
}
