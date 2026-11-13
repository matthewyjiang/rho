//! A stand-in for the daemon host, so transport tests need no app code.

use std::sync::Arc;

use rho_protocol::{
    BuildId, CredentialStoreKind, CredentialStoreStatus, DaemonStatus, ErrorCode, JobListResponse,
    RpcError, Service, SettingsService, StoreSelectedBy, PROTOCOL_VERSION,
};

use super::{
    paths::DaemonPaths,
    platform::Listener,
    shutdown::{Ending, Shutdown},
};

pub(crate) use super::platform::StopTarget;

/// A stand-in for a daemon that refuses every client: it listens where the
/// daemon of `paths` would, and answers a hello with a version mismatch.
pub(crate) struct RefusingListener(Listener);

impl RefusingListener {
    /// Listens on the endpoint of `paths`, preparing its runtime directory.
    pub(crate) fn bind(paths: &DaemonPaths) -> Self {
        paths.prepare_runtime_dir().unwrap();
        Self(Listener::bind(paths).unwrap())
    }

    /// Answers the next client's hello with `refusal`.
    pub(crate) async fn refuse_next(&mut self, refusal: rho_protocol::VersionMismatch) {
        use rho_protocol::{Message, MessageReader, MessageWriter, Response};

        let (read, write) = tokio::io::split(self.0.accept().await.unwrap());
        let hello = MessageReader::new(read).read_line().await.unwrap();
        let Message::Request(hello) = Message::parse(&hello.expect("a hello")).unwrap() else {
            panic!("expected a request");
        };
        let refused = RpcError::new(ErrorCode::VersionMismatch, refusal.to_string())
            .with_data(serde_json::to_value(refusal).unwrap());
        // The client may leave as soon as it has read the refusal.
        let _ = MessageWriter::new(write)
            .write(&Message::Response(Response {
                id: Some(hello.id),
                outcome: Err(refused),
            }))
            .await;
    }
}

/// Answers `status` with [`test_status`] and, like the real host, stops the
/// daemon on `rho/daemon/stop` by requesting its shutdown. It hosts no
/// sessions, and leaves every other method to [`Service`]'s defaults.
pub(super) struct StubHost {
    shutdown: Shutdown,
}

impl StubHost {
    pub(super) fn new(shutdown: &Shutdown) -> Arc<Self> {
        Arc::new(Self {
            shutdown: shutdown.clone(),
        })
    }
}

/// What restarting [`StubHost`] costs: nobody uses it.
const UNUSED: rho_protocol::RestartCost = rho_protocol::RestartCost {
    in_progress: 0,
    clients: 0,
};

/// What [`StubHost`] reports, distinct from any real daemon's status.
pub(super) fn test_status() -> DaemonStatus {
    DaemonStatus {
        pid: 0,
        version: "test".to_owned(),
        protocol: PROTOCOL_VERSION,
        started_at: 0,
        uptime_secs: 0,
        jobs: 0,
        in_progress: 0,
        socket: "/test/daemon.sock".into(),
        credential_store: CredentialStoreStatus {
            store: CredentialStoreKind::File,
            selected_by: StoreSelectedBy::Environment,
            unreachable: None,
        },
        upgrade: None,
    }
}

impl Service for StubHost {
    fn refused_newer(&self, _: rho_protocol::ProtocolVersion) -> rho_protocol::RestartCost {
        UNUSED
    }

    fn skewed(&self, _: &rho_protocol::ClientInfo, _: &BuildId) -> rho_protocol::RestartCost {
        UNUSED
    }

    async fn restart(&self) {
        self.shutdown.request(Ending::Restart, |_| {});
    }

    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {
        self.shutdown.request(Ending::Stop, |_| {});
    }

    async fn list_jobs(&self) -> JobListResponse {
        JobListResponse { jobs: Vec::new() }
    }
}

impl SettingsService for StubHost {}
