//! What the rho daemon answers over `rho-protocol`.
//!
//! `crate::daemon` is the generic transport (socket, lock, accept loop) and
//! never learns what a session is; [`DaemonHost`] is the [`Service`] it routes
//! requests to, and `app::daemon_cli` wires the two together. Thread hosting
//! (the job registry, one session runtime per thread, event fan-out) lands
//! here.

use std::{future::Future, path::PathBuf, time::Instant};

use rho_protocol::{DaemonStatus, Service, PROTOCOL_VERSION};
use rho_sdk::CancellationToken;

/// The daemon's protocol service.
pub(super) struct DaemonHost {
    socket: PathBuf,
    started_at: u64,
    started: Instant,
    shutdown: CancellationToken,
}

impl DaemonHost {
    /// A host for the daemon listening on `socket` since `started_at` (Unix
    /// seconds). `rho/daemon/stop` cancels `shutdown`, which ends the
    /// transport's serve loop.
    pub(super) fn new(socket: PathBuf, started_at: u64, shutdown: CancellationToken) -> Self {
        Self {
            socket,
            started_at,
            started: Instant::now(),
            shutdown,
        }
    }
}

impl Service for DaemonHost {
    fn status(&self) -> impl Future<Output = DaemonStatus> + Send {
        async move {
            DaemonStatus {
                pid: std::process::id(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                protocol: PROTOCOL_VERSION,
                started_at: self.started_at,
                uptime_secs: self.started.elapsed().as_secs(),
                // Counts jobs once the job registry lands with thread hosting.
                jobs: 0,
                socket: self.socket.clone(),
            }
        }
    }

    fn stop(&self) -> impl Future<Output = ()> + Send {
        async move {
            tracing::info!("stop requested");
            self.shutdown.cancel();
        }
    }
}
