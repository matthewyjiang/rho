use pretty_assertions::assert_eq;
use rho_sdk::{HostChoice, HostInputRequest, HostQuestion, SelectionMode, SessionId};
use tokio::sync::oneshot;

#[cfg(unix)]
use crate::herdr::test_support::{reporter_for_socket, TestHerdrServer};

use super::ParentActivity;
use crate::{
    app::subagent_host_input::SubagentHostInputRequest,
    tui::{tests::test_app, ComposerMode},
};

fn host_request() -> HostInputRequest {
    HostInputRequest::questionnaire(
        "child question",
        vec![HostQuestion::new(
            "color",
            "Choose one color",
            vec![HostChoice::new("blue", "blue")],
            SelectionMode::One,
        )
        .unwrap()],
    )
    .unwrap()
}

fn pending_request(
    parent_session_id: SessionId,
) -> (
    SubagentHostInputRequest,
    oneshot::Receiver<Result<rho_sdk::HostInputResponse, rho_sdk::Error>>,
) {
    let (response, receiver) = oneshot::channel();
    (
        SubagentHostInputRequest {
            run_id: "abc123".into(),
            agent_id: "worker".into(),
            parent_session_id,
            request: host_request(),
            response,
        },
        receiver,
    )
}

#[tokio::test]
async fn questionnaire_from_another_parent_session_is_rejected() {
    let mut app = test_app();
    let current_session = SessionId::new();
    let (request, response) = pending_request(SessionId::new());
    app.queued_subagent_questionnaires.push_back(request);

    assert!(app
        .poll_subagent_questionnaires(&current_session)
        .await
        .unwrap());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert!(app.queued_subagent_questionnaires.is_empty());
    let error = response.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("parent session changed"));
}

#[tokio::test]
async fn cancelled_queued_questionnaire_is_not_presented() {
    let mut app = test_app();
    let session_id = SessionId::new();
    let (request, response) = pending_request(session_id.clone());
    drop(response);
    app.queued_subagent_questionnaires.push_back(request);

    assert!(app
        .poll_running_subagent_questionnaires(&session_id)
        .await
        .unwrap());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert!(app.queued_subagent_questionnaires.is_empty());
}

#[tokio::test]
async fn queued_questionnaire_waits_without_clearing_parent_draft() {
    let mut app = test_app();
    let session_id = SessionId::new();
    let (request, _response) = pending_request(session_id.clone());
    app.input_ui.set_text_and_cursor("unsent draft".into(), 12);
    app.queued_subagent_questionnaires.push_back(request);

    assert!(!app
        .poll_running_subagent_questionnaires(&session_id)
        .await
        .unwrap());

    assert_eq!(app.input_ui.text(), "unsent draft");
    assert_eq!(app.input_ui.cursor(), 12);
    assert!(app.pending_subagent_questionnaire.is_none());
    assert_eq!(app.queued_subagent_questionnaires.len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn answered_running_questionnaire_reports_parent_working_again() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let socket_dir = tempfile::tempdir().unwrap();
    let socket_path = socket_dir.path().join("herdr.sock");
    let mut server = TestHerdrServer::bind(&socket_path).await;
    let mut app = test_app();
    app.info.services.herdr = reporter_for_socket(&socket_path);
    app.info.session.session_id = Some("parent-session".into());
    let session_id = SessionId::new();
    let (request, response) = pending_request(session_id.clone());
    app.queued_subagent_questionnaires.push_back(request);

    app.poll_running_subagent_questionnaires(&session_id)
        .await
        .unwrap();
    let blocked = server.next_request().await;
    assert_eq!(blocked["params"]["state"], "blocked");

    app.handle_questionnaire_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    app.poll_running_subagent_questionnaires(&session_id)
        .await
        .unwrap();

    let working = server.next_request().await;
    assert_eq!(working["params"]["state"], "working");
    assert_eq!(app.status, "running");
    assert!(response.await.unwrap().is_ok());
}

#[tokio::test]
async fn cancelled_visible_questionnaire_restores_input_composer() {
    let mut app = test_app();
    let session_id = SessionId::new();
    let (request, response) = pending_request(session_id.clone());
    app.queued_subagent_questionnaires.push_back(request);
    app.present_next_subagent_questionnaire(&session_id)
        .await
        .unwrap();
    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::Questionnaire(_)
    ));

    drop(response);
    assert!(app
        .finish_pending_subagent_questionnaire(ParentActivity::Idle)
        .await
        .unwrap());

    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert!(app.pending_subagent_questionnaire.is_none());
    assert_eq!(app.status, "ready");
}
