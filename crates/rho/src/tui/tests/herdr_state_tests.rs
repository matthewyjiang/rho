#![cfg(unix)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalDecision, ApprovalRequest, CapabilityRequest, CapabilitySource, PathScope,
    PendingApproval,
};

use crate::{
    herdr::test_support::{reporter_for_socket, TestHerdrServer},
    questionnaire::QuestionnaireQuestionKind,
};

use super::super::{
    approval::ApprovalKeyOutcome,
    questionnaire::{
        QuestionAnswerRequest, QuestionnaireQuestion, QuestionnaireRequest,
        QuestionnaireResponseChannel,
    },
};
use super::*;

#[tokio::test]
async fn opening_questionnaire_reports_blocked_and_resume_reports_working() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = TestHerdrServer::bind(&socket_path).await;
    let mut bootstrap = test_bootstrap();
    bootstrap.services.herdr = reporter_for_socket(&socket_path);
    bootstrap.session.session_id = Some("session-questionnaire".into());
    let mut app = App::new(bootstrap, crate::herdr::HerdrGraphicsCapability::NotHerdr);
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();

    app.open_questionnaire(QuestionAnswerRequest {
        request: QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![QuestionnaireQuestion {
                id: "choice".into(),
                question: "Pick one".into(),
                header: None,
                help: None,
                default: None,
                kind: QuestionnaireQuestionKind::Choice,
                required: true,
                choices: vec!["alpha".into(), "beta".into()],
                allow_other: false,
            }],
        },
        response: QuestionnaireResponseChannel::new(reply_tx),
    })
    .await
    .unwrap();

    let blocked = server.next_request().await;
    assert_eq!(blocked["method"], "pane.report_agent");
    assert_eq!(blocked["params"]["state"], "blocked");
    assert_eq!(blocked["params"]["message"], "waiting for your answers");
    assert_eq!(
        blocked["params"]["agent_session_id"],
        "session-questionnaire"
    );
    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::Questionnaire(_)
    ));

    app.report_herdr_working().await;
    let working = server.next_request().await;
    assert_eq!(working["params"]["state"], "working");
    assert!(working["params"].get("message").is_none());
}

#[tokio::test]
async fn resolving_approval_reports_blocked_then_signals_resume() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = TestHerdrServer::bind(&socket_path).await;
    let mut bootstrap = test_bootstrap();
    bootstrap.services.herdr = reporter_for_socket(&socket_path);
    bootstrap.session.session_id = Some("session-approval".into());
    let mut app = App::new(bootstrap, crate::herdr::HerdrGraphicsCapability::NotHerdr);
    let (pending, mut decision_rx) = test_pending_approval();

    app.open_approval(pending).await;

    let blocked = server.next_request().await;
    assert_eq!(blocked["params"]["state"], "blocked");
    assert_eq!(blocked["params"]["message"], "waiting for approval");
    assert!(matches!(app.input_ui.composer(), ComposerMode::Approval(_)));

    let outcome = app
        .handle_approval_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), 80)
        .unwrap();
    assert_eq!(outcome, ApprovalKeyOutcome::Resolved);
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(decision_rx.try_recv(), Ok(ApprovalDecision::AllowOnce));

    // prompt_turn maps ApprovalResolved onto this resume report.
    app.report_herdr_working().await;
    let working = server.next_request().await;
    assert_eq!(working["params"]["state"], "working");
    assert!(working["params"].get("message").is_none());
}

#[tokio::test]
async fn esc_approval_resolves_without_requiring_working_resume() {
    let mut app = test_app();
    let (pending, mut decision_rx) = test_pending_approval();
    app.open_approval(pending).await;

    let outcome = app
        .handle_approval_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), 80)
        .unwrap();
    assert_eq!(outcome, ApprovalKeyOutcome::Resolved);
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(
        decision_rx.try_recv(),
        Ok(ApprovalDecision::Deny {
            reason: "cancelled by user".into()
        })
    );
    // Esc during a turn becomes StreamControl::Interrupt in the event loop, so
    // herdr stays blocked until report_resting_herdr_state at turn end.
}

#[tokio::test]
async fn resting_herdr_state_stays_blocked_when_auth_is_unavailable() {
    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = TestHerdrServer::bind(&socket_path).await;
    let mut bootstrap = test_bootstrap();
    bootstrap.services.herdr = reporter_for_socket(&socket_path);
    bootstrap.services.auth_unavailable = Some("login required".into());
    let app = App::new(bootstrap, crate::herdr::HerdrGraphicsCapability::NotHerdr);

    app.report_resting_herdr_state().await;

    let request = server.next_request().await;
    assert_eq!(request["params"]["state"], "blocked");
    assert_eq!(request["params"]["message"], "login required");
}

fn test_pending_approval() -> (
    PendingApproval,
    tokio::sync::oneshot::Receiver<ApprovalDecision>,
) {
    PendingApproval::new(ApprovalRequest::new(
        CapabilityRequest::write_path(
            "src/main.rs",
            PathScope::PrimaryWorkspace,
            CapabilitySource::built_in_tool("bash"),
        ),
        "needs editing",
    ))
}
