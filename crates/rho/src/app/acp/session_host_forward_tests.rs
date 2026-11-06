use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{atomic::AtomicU64, Arc, Mutex},
    time::Duration,
};

use agent_client_protocol::{
    schema::v1::{
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionId,
        SessionNotification, SessionUpdate,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ModelIdentity, provider::ScriptedProvider, ApprovalDecision, ApprovalRequest,
    CapabilityRequest, CapabilitySource, PathScope, RunEvent, SessionOptions,
};
use tokio::sync::{mpsc, oneshot};

use super::Forwarder;
use crate::{
    app::acp::{events::EventMapper, signals::SessionSignals, AcpClientPort, HostFeatures},
    session_runtime::{cancelled_approval, ApprovalId, SessionEvent, TurnControl},
    tools::todo::TodoState,
};

/// Holds its permission request open until released, and passes every
/// update it gets to the test.
struct HoldingPermissionPort {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    updates: mpsc::UnboundedSender<SessionUpdate>,
}

impl AcpClientPort for HoldingPermissionPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        let _ = self.updates.send(notification.update);
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async move {
            let entered = self.entered.lock().expect("entered").take();
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            let release = self.release.lock().expect("release").take();
            if let Some(release) = release {
                let _ = release.await;
            }
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ))
        })
    }
}

async fn session() -> rho_sdk::Session {
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::new(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "model"),
            Vec::new(),
        )))
        .build()
        .unwrap();
    runtime.session(SessionOptions::new()).await.unwrap()
}

// Covers: a host permission prompt must not stall event delivery, and the
// host's reply must reach the turn as the answer to that approval.
// Owner: ACP turn forwarder
#[tokio::test]
async fn permission_request_does_not_block_event_drain() {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (updates_tx, mut updates) = mpsc::unbounded_channel();
    let port = HoldingPermissionPort {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
        updates: updates_tx,
    };
    let session_id = SessionId::new("session-1");
    let placeholders = AtomicU64::new(0);
    let forwarder = Forwarder {
        session_id: &session_id,
        client: &port,
        features: HostFeatures::default(),
        mapper: EventMapper::new(HostFeatures::default(), PathBuf::new(), Default::default()),
        signals: SessionSignals::new(session().await, TodoState::default(), None, Vec::new()),
        placeholders: &placeholders,
    };
    let (events, event_receiver) = mpsc::unbounded_channel();
    let (control_sender, mut control) = mpsc::unbounded_channel();

    let turn = async {
        events
            .send(SessionEvent::ApprovalRequested {
                id: ApprovalId::nth_for_tests(0),
                request: ApprovalRequest::new(
                    CapabilityRequest::write_path(
                        "/workspace/file.rs",
                        PathScope::PrimaryWorkspace,
                        CapabilitySource::built_in_tool("write"),
                    ),
                    "edit file",
                ),
            })
            .unwrap();
        entered_rx.await.expect("permission started");
        events
            .send(SessionEvent::Run(RunEvent::AssistantTextDelta {
                text: "hi".into(),
            }))
            .unwrap();
        // A tripwire for a stalled forwarder; delivery itself is immediate.
        let update = tokio::time::timeout(Duration::from_secs(10), updates.recv())
            .await
            .expect("event must be delivered while the permission request is still pending")
            .expect("an update");
        assert!(
            matches!(update, SessionUpdate::AgentMessageChunk(_)),
            "{update:?}"
        );

        release_tx.send(()).expect("release permission");
        let answer = match control.recv().await.expect("the forwarder answers") {
            TurnControl::RespondApproval { id, decision } => Some((id, decision)),
            TurnControl::AnswerPlan { .. }
            | TurnControl::AnswerQuestion { .. }
            | TurnControl::HostFailed { .. } => None,
        };
        let expected: Option<(ApprovalId, ApprovalDecision)> =
            Some((ApprovalId::nth_for_tests(0), cancelled_approval()));
        assert_eq!(answer, expected);
        drop(events);
    };
    let ((), forwarded) = tokio::join!(turn, forwarder.forward(event_receiver, control_sender));
    assert_eq!(forwarded.failure, None);
}
