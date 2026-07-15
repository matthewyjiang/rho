use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    questionnaire::QuestionnaireQuestionKind,
    tui::questionnaire::{QuestionnaireQuestion, QuestionnaireRequest},
};

use super::*;

#[test]
fn second_ctrl_c_cancels_questionnaire_without_exiting_tui() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut app = test_app();
    app.composer = ComposerMode::Questionnaire(QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![QuestionnaireQuestion {
                id: "answer".into(),
                question: "Continue?".into(),
                help: None,
                default: None,
                kind: QuestionnaireQuestionKind::Confirm,
                required: true,
                choices: Vec::new(),
                allow_other: false,
            }],
        },
        QuestionnaireResponseChannel::new(reply_tx),
    ));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

    assert!(app.handle_questionnaire_key(ctrl_c).unwrap());
    assert_eq!(app.status, "answer cleared; press ctrl-c again to cancel");
    assert!(app.handle_questionnaire_key(ctrl_c).unwrap());

    assert!(matches!(app.composer, ComposerMode::Input));
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
