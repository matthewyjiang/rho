//! A stand-in for the daemon host, so transport tests need no app code.

use std::{future::Future, sync::Arc};

use rho_protocol::{DaemonStatus, Service, PROTOCOL_VERSION};
use rho_sdk::CancellationToken;

/// Answers `status` with [`test_status`] and, like the real host, stops the
/// daemon on `rho/daemon/stop` by cancelling its shutdown token.
pub(super) struct StopService {
    shutdown: CancellationToken,
}

impl StopService {
    pub(super) fn new(shutdown: &CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            shutdown: shutdown.clone(),
        })
    }
}

/// What [`StopService`] reports, distinct from any real daemon's status.
pub(super) fn test_status() -> DaemonStatus {
    DaemonStatus {
        pid: 0,
        version: "test".to_owned(),
        protocol: PROTOCOL_VERSION,
        started_at: 0,
        uptime_secs: 0,
        jobs: 0,
        socket: "/test/daemon.sock".into(),
    }
}

impl Service for StopService {
    fn status(&self) -> impl Future<Output = DaemonStatus> + Send {
        std::future::ready(test_status())
    }

    fn stop(&self) -> impl Future<Output = ()> + Send {
        async move { self.shutdown.cancel() }
    }
}
