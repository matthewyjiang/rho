use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    BoundaryInputRequest, InputBoundary, Rho, RunEvent, SessionOptions, UserInput,
};

use super::HeadlessDelegation;
use crate::{
    app::{agent_executor::AgentRunHandle, SubagentManager},
    subagent::{RunState, RunStatus},
};

struct Fixture {
    _root: tempfile::TempDir,
    session: rho_sdk::Session,
    manager: SubagentManager,
    host: HeadlessDelegation,
    run: rho_sdk::Run,
    status: tokio::sync::watch::Sender<RunStatus>,
    completion: tokio::sync::watch::Sender<bool>,
    child_cancel: rho_tools::cancellation::RunCancellation,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let manager = SubagentManager::new(
            crate::config::Config::default(),
            root.path().join("config.toml"),
            root.path().to_path_buf(),
        );
        let runtime = Rho::builder()
            .provider(ScriptedProvider::new(
                ModelIdentity::new("scripted", "test", "model"),
                ["initial answer", "child result incorporated"].map(|text| {
                    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                        text.into(),
                    )]))
                }),
            ))
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        let host = HeadlessDelegation::attach(&session, Some(&manager), None)
            .unwrap()
            .unwrap();
        let (status, status_rx) = tokio::sync::watch::channel(RunStatus {
            state: RunState::Running,
            ..Default::default()
        });
        let (completion, completion_rx) = tokio::sync::watch::channel(false);
        let child_cancel = rho_tools::cancellation::RunCancellation::new();
        manager.insert_handle_for_test(
            "abc123",
            session.id().as_str(),
            AgentRunHandle::controlled_for_test(status_rx, completion_rx, child_cancel.clone()),
        );
        let run = session
            .start(UserInput::text("delegate work"))
            .await
            .unwrap();
        Self {
            _root: root,
            session,
            manager,
            host,
            run,
            status,
            completion,
            child_cancel,
        }
    }

    async fn final_boundary(&mut self) -> BoundaryInputRequest {
        let first = self.host.requests.recv().await.unwrap();
        assert_eq!(first.boundary(), InputBoundary::BeforeProvider);
        self.host.respond(first).await;
        let request = loop {
            tokio::select! {
                request = self.host.requests.recv() => break request.unwrap(),
                event = self.run.next_event() => assert!(!matches!(event, None | Some(RunEvent::Completed { .. }))),
            }
        };
        assert_eq!(request.boundary(), InputBoundary::BeforeCompletion);
        request
    }
}

// Covers: a live child holds natural completion, then its result continues the
// original parent run exactly once. The SDK channel tests do not own this wait.
// Owner: shared CLI/ACP delegation lifecycle.
#[tokio::test]
async fn child_completion_at_final_boundary_continues_parent_once() {
    let mut fixture = Fixture::new().await;
    let request = fixture.final_boundary().await;
    {
        let reply = fixture.host.respond(request);
        tokio::pin!(reply);
        assert!(futures_util::poll!(&mut reply).is_pending());
        fixture.status.send_replace(RunStatus {
            state: RunState::Ok,
            result: Some("child finding".into()),
            ..Default::default()
        });
        fixture.completion.send_replace(true);
        reply.await;
    }
    let mut host = Some(fixture.host);
    let applied = HeadlessDelegation::drive(&mut host, async {
        let mut applied = 0;
        while let Some(event) = fixture.run.next_event().await {
            if matches!(event, RunEvent::BoundaryInputApplied { .. }) {
                applied += 1;
            }
        }
        applied
    })
    .await;
    assert_eq!(
        fixture.run.outcome().await.unwrap().text(),
        "child result incorporated"
    );
    assert_eq!(applied, 1);
    assert!(fixture
        .manager
        .take_notifications(fixture.session.id().as_str())
        .is_empty());
}

// Covers: cancellation while natural completion is held does not wait for a
// child result or issue another provider request; teardown cancels the child.
// Owner: shared CLI/ACP delegation lifecycle.
#[tokio::test]
async fn cancellation_interrupts_child_wait_and_shutdown_stops_child() {
    let mut fixture = Fixture::new().await;
    let request = fixture.final_boundary().await;
    let waiting = fixture.host.respond(request);
    tokio::pin!(waiting);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    assert!(matches!(
        fixture.run.outcome().await,
        Err(rho_sdk::Error::Cancelled)
    ));
    let shutdown = fixture.manager.shutdown();
    tokio::pin!(shutdown);
    assert!(futures_util::poll!(&mut shutdown).is_pending());
    fixture.child_cancel.cancelled().await;
    fixture.status.send_replace(RunStatus {
        state: RunState::Stopped,
        ..Default::default()
    });
    fixture.completion.send_replace(true);
    shutdown.await;
    assert_eq!(
        fixture.manager.status("abc123").unwrap().status.state,
        RunState::Stopped
    );
}

// Covers: a cancelled SDK boundary must restore a result it did not accept.
// Owner: shared CLI/ACP delivery receipts.
#[tokio::test]
async fn rejected_boundary_restores_terminal_result() {
    let mut fixture = Fixture::new().await;
    let request = fixture.final_boundary().await;
    fixture.status.send_replace(RunStatus {
        state: RunState::Ok,
        result: Some("child finding".into()),
        ..Default::default()
    });
    fixture.completion.send_replace(true);
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    assert!(matches!(
        fixture.run.outcome().await,
        Err(rho_sdk::Error::Cancelled)
    ));
    fixture.host.respond(request).await;
    let notifications = fixture
        .manager
        .take_notifications(fixture.session.id().as_str());
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].snapshot.id, "abc123");
}

// Covers: ending the event pump immediately after input commits must still
// reap its receipt, or the next prompt would receive the same result again.
// Owner: shared CLI/ACP delivery receipts.
#[tokio::test]
async fn accepted_boundary_is_reaped_when_event_pump_returns() {
    let mut fixture = Fixture::new().await;
    fixture.status.send_replace(RunStatus {
        state: RunState::Ok,
        result: Some("child finding".into()),
        ..Default::default()
    });
    fixture.completion.send_replace(true);
    let mut host = Some(fixture.host);
    HeadlessDelegation::drive(&mut host, async {
        loop {
            match fixture.run.next_event().await {
                Some(RunEvent::BoundaryInputApplied { .. }) => break,
                Some(_) => {}
                None => panic!("run ended before child delivery"),
            }
        }
    })
    .await;
    assert!(fixture
        .manager
        .take_notifications(fixture.session.id().as_str())
        .is_empty());
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    let _ = fixture.run.outcome().await;
}

// Covers: routine notices cannot wake an idle parent, while an action request
// releases the held boundary even though the requesting child is still alive.
// Owner: shared CLI/ACP notice scheduling.
#[tokio::test]
async fn action_request_wakes_parent_and_includes_earlier_notices() {
    use crate::app::subagent_messaging::{NoticeDelivery, SubagentNotice};

    let mut fixture = Fixture::new().await;
    let request = fixture.final_boundary().await;
    let ordinary = SubagentNotice {
        run_id: "abc123".into(),
        agent_id: "fixture".into(),
        parent_session_id: fixture.session.id().clone(),
        message: "finding".into(),
        delivery: NoticeDelivery::NextTurn,
        acknowledged: Default::default(),
    };
    fixture.manager.post_notice_for_test(ordinary.clone());
    let action = SubagentNotice {
        message: "need parent decision".into(),
        delivery: NoticeDelivery::ParentActionRequired,
        acknowledged: Default::default(),
        ..ordinary.clone()
    };
    {
        let reply = fixture.host.respond(request);
        tokio::pin!(reply);
        assert!(futures_util::poll!(&mut reply).is_pending());
        fixture.manager.post_notice_for_test(action.clone());
        reply.await;
    }
    assert!(ordinary.is_acknowledged());
    assert!(action.is_acknowledged());
    assert!(fixture
        .manager
        .has_running_for_session(fixture.session.id().as_str()));
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    assert!(matches!(
        fixture.run.outcome().await,
        Err(rho_sdk::Error::Cancelled)
    ));
}
