use std::{future::Future, pin::Pin, sync::Mutex};

use agent_client_protocol::{
    schema::v1::{
        ConfigOptionUpdate, RequestPermissionRequest, RequestPermissionResponse,
        SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_sdk::{model::ModelIdentity, provider::ScriptedProvider};

use super::*;
use crate::{
    app::acp::{agent::RhoAcpAgent, session_host::SessionHost, AcpClientPort, AcpStartup},
    config::Config,
    permission::PermissionMode,
};

#[derive(Default)]
struct RecordingPort(Mutex<Vec<SessionNotification>>);

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

// Covers: settings arriving after replacement publication must wait until the
// initial response/commands are sent; a failed initial delivery must be retried.
// Owner: ACP session publication, with the wire handoff held explicitly.
#[tokio::test]
async fn settings_wait_for_initial_advertisements_and_retry_failed_delivery() {
    for sent in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let startup = AcpStartup::for_tests(
            Config {
                permission_mode: PermissionMode::Plan,
                ..Config::default()
            },
            root.path(),
        );
        let mut host = SessionHost::unsaved_for_tests(
            &startup,
            root.path(),
            rho_sdk::SessionId::new(),
            Arc::new(ScriptedProvider::new(
                ModelIdentity::new("test", "fixture", "start"),
                Vec::new(),
            )),
        )
        .await;
        let commands = host.available_commands(&startup.config);
        let session_id = commands.session_id.clone();
        let agent = Arc::new(RhoAcpAgent::new(startup));
        let live = LiveSession::new(host);
        let publication = PublicationGuard::new(&live, &session_id).await;
        agent.publish(session_id.clone(), live).await;

        // The slot is already visible, as it is during old-host teardown in
        // load_session, but no update may precede the load's initial payloads.
        let port = RecordingPort::default();
        let change = agent.set_config_option(
            SetSessionConfigOptionRequest::new(session_id.clone(), "mode", "bypass"),
            &port,
        );
        tokio::pin!(change);
        assert!(futures_util::poll!(&mut change).is_pending());
        assert_eq!(*port.0.lock().unwrap(), Vec::<SessionNotification>::new());
        if sent {
            publication.sent();
        } else {
            drop(publication);
        }
        let response = change.await.unwrap();
        let mut expected = vec![SessionNotification::new(
            session_id,
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(response.config_options)),
        )];
        if !sent {
            expected.push(commands);
        }
        assert_eq!(*port.0.lock().unwrap(), expected, "initial sent: {sent}");
        agent.shutdown_all().await;
    }
}
