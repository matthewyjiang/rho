use pretty_assertions::assert_eq;
use rho_sdk::{HostChoice, HostInputRequest, HostQuestion, SelectionMode, SessionId};
use tokio::sync::oneshot;

#[cfg(unix)]
use crate::herdr::test_support::{reporter_for_socket, TestHerdrServer};

use super::ParentActivity;
use crate::{
    app::subagent_host_input::SubagentHostInputRequest,
    tui::{
        goal::{GoalEvaluation, GoalState},
        tests::test_app,
        ComposerMode,
    },
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

#[test]
fn questionnaire_presentation_allows_only_blocked_goals() {
    let mut app = test_app();
    assert!(app.can_present_subagent_questionnaire());

    app.goal = Some(GoalState::new("tests pass".into()));
    assert!(!app.can_present_subagent_questionnaire());

    app.goal
        .as_mut()
        .unwrap()
        .record_evaluation(&GoalEvaluation::Blocked {
            reason: "human input required".into(),
            pending_steps: Vec::new(),
        });
    assert!(app.can_present_subagent_questionnaire());
}

#[tokio::test]
async fn questionnaire_from_another_parent_session_is_rejected() {
    let mut app = test_app();
    let current_session = SessionId::new();
    let (request, response) = pending_request(SessionId::new());
    app.subagent_inbox.push_questionnaire_for_test(request);

    assert!(app
        .poll_subagent_questionnaires(&current_session)
        .await
        .unwrap());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(app.subagent_inbox.queued_questionnaire_count(), 0);
    let error = response.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("parent session changed"));
}

#[tokio::test]
async fn cancelled_queued_questionnaire_is_not_presented() {
    let mut app = test_app();
    let session_id = SessionId::new();
    let (request, response) = pending_request(session_id.clone());
    drop(response);
    app.subagent_inbox.push_questionnaire_for_test(request);

    assert!(app.poll_subagent_questionnaires(&session_id).await.unwrap());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(app.subagent_inbox.queued_questionnaire_count(), 0);
}

#[tokio::test]
async fn queued_questionnaire_waits_without_clearing_parent_draft() {
    let mut app = test_app();
    let session_id = SessionId::new();
    let (request, _response) = pending_request(session_id.clone());
    app.input_ui.set_text_and_cursor("unsent draft".into(), 12);
    app.subagent_inbox.push_questionnaire_for_test(request);

    assert!(!app.poll_subagent_questionnaires(&session_id).await.unwrap());

    assert_eq!(app.input_ui.text(), "unsent draft");
    assert_eq!(app.input_ui.cursor(), 12);
    assert!(app.pending_subagent_questionnaire.is_none());
    assert_eq!(app.subagent_inbox.queued_questionnaire_count(), 1);
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
    app.subagent_inbox.push_questionnaire_for_test(request);

    app.poll_subagent_questionnaires(&session_id).await.unwrap();
    let blocked = server.next_request().await;
    assert_eq!(blocked["params"]["state"], "blocked");

    app.handle_questionnaire_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    app.finish_pending_subagent_questionnaire(ParentActivity::Working("running"))
        .await
        .unwrap();

    let working = server.next_request().await;
    assert_eq!(working["params"]["state"], "working");
    assert_eq!(app.status(), "running");
    assert!(response.await.unwrap().is_ok());
}

#[tokio::test]
async fn answered_goal_questionnaire_restores_wait_status() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = test_app();
    let session_id = SessionId::new();
    let (request, response) = pending_request(session_id.clone());
    app.subagent_inbox.push_questionnaire_for_test(request);

    app.poll_waiting_subagent_questionnaires(&session_id)
        .await
        .unwrap();
    app.handle_questionnaire_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    app.poll_waiting_subagent_questionnaires(&session_id)
        .await
        .unwrap();

    assert_eq!(app.status(), "waiting for delegated agents");
    assert!(response.await.unwrap().is_ok());
}

#[tokio::test]
async fn cancelled_visible_questionnaire_restores_input_composer() {
    let mut app = test_app();
    let session_id = SessionId::new();
    let (request, response) = pending_request(session_id.clone());
    app.subagent_inbox.push_questionnaire_for_test(request);
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
    assert_eq!(app.status(), "ready");
}
