use std::{
    future::Future,
    pin::Pin,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use agent_client_protocol::{
    schema::v1::{
        RequestPermissionRequest, RequestPermissionResponse, SessionId, SessionInfoUpdate,
        SessionNotification, SessionUpdate,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;

use super::super::{LiveSession, PromptGate, RhoAcpAgent};
use crate::{
    app::acp::{AcpClientPort, AcpStartup},
    config::Config,
};

#[derive(Default)]
struct RecordingPort(Mutex<Vec<SessionNotification>>);

impl RecordingPort {
    fn take(&self) -> Vec<SessionNotification> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl AcpClientPort for RecordingPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        self.0.lock().unwrap().push(notification);
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async { Err(AcpError::method_not_found()) })
    }
}

fn vacant_session() -> Arc<LiveSession> {
    Arc::new(LiveSession {
        host: Arc::new(tokio::sync::Mutex::new(None)),
        cancel: Arc::new(PromptGate::new()),
        replaced: Arc::new(AtomicBool::new(false)),
    })
}

/// What happens to the asking host while a load, close, or delete of its
/// session holds the ID.
#[derive(Clone, Copy, Debug)]
enum Meanwhile {
    /// A load that failed: the asking host stays published.
    LoadFailed,
    Closed,
    /// A load that published a successor under the same ID.
    Reloaded,
}

// Covers: a generated title must wait out an in-flight session/load (hosts
// such as T3 Code discard non-replay updates then), and must reach only the
// host that asked for it: never a successor a reload published under the
// same ID, nor a session closed meanwhile.
// Owner: ACP session titles
#[tokio::test]
async fn title_waits_out_loads_and_reaches_only_the_asking_host() {
    let home = tempfile::tempdir().unwrap();
    let title = "Rename the widget";
    for (meanwhile, sent) in [
        (Meanwhile::LoadFailed, true),
        (Meanwhile::Closed, false),
        (Meanwhile::Reloaded, false),
    ] {
        let agent = RhoAcpAgent::new(AcpStartup::for_tests(Config::default(), home.path()));
        let id = SessionId::new("titled");
        let asking = vacant_session();
        agent
            .sessions
            .lock()
            .await
            .insert(id.clone(), Arc::clone(&asking));
        let port = RecordingPort::default();
        let install = agent.install_lock_for(&id).await;
        let loading = install.lock().await;

        let mut report =
            std::pin::pin!(agent.send_title(&id, &asking.cancel, title.to_owned(), &port));
        assert!(
            futures_util::poll!(report.as_mut()).is_pending(),
            "{meanwhile:?}: the title must wait for the load"
        );
        match meanwhile {
            Meanwhile::LoadFailed => {}
            Meanwhile::Closed => {
                agent.sessions.lock().await.remove(&id);
            }
            Meanwhile::Reloaded => {
                agent
                    .sessions
                    .lock()
                    .await
                    .insert(id.clone(), vacant_session());
            }
        }
        drop(loading);
        report.await;

        let expected = sent
            .then(|| {
                SessionNotification::new(
                    id.clone(),
                    SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title)),
                )
            })
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(port.take(), expected, "{meanwhile:?}");
    }
}
