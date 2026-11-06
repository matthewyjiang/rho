//! What the rho daemon answers over `rho-protocol`.
//!
//! `crate::daemon` is the generic transport (socket, lock, accept loop) and
//! never learns what a session is; [`DaemonHost`] is the [`Service`] it routes
//! requests to, and `app::daemon_cli` wires the two together. The host owns
//! the job registry (`jobs/`), whose records outlive the daemon.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use rho_protocol::{DaemonStatus, Service, PROTOCOL_VERSION};
use rho_sdk::CancellationToken;

mod jobs;

use jobs::{JobRegistry, JobStore};

/// The daemon's protocol service.
pub(super) struct DaemonHost {
    socket: PathBuf,
    started_at: u64,
    started: Instant,
    shutdown: CancellationToken,
    jobs: JobRegistry,
}

impl DaemonHost {
    /// A host for the daemon listening on `socket` since `started_at` (Unix
    /// seconds), keeping its job records in `rho_home`. `rho/daemon/stop`
    /// cancels `shutdown`, which ends the transport's serve loop.
    ///
    /// Opening the registry rewrites the records of turns an earlier daemon
    /// left open, so call this only while holding the daemon lock.
    pub(super) fn new(
        socket: PathBuf,
        started_at: u64,
        shutdown: CancellationToken,
        rho_home: &Path,
    ) -> Self {
        Self {
            socket,
            started_at,
            started: Instant::now(),
            shutdown,
            jobs: JobRegistry::open(JobStore::in_rho_home(rho_home)),
        }
    }
}

impl Service for DaemonHost {
    async fn status(&self) -> DaemonStatus {
        DaemonStatus {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: PROTOCOL_VERSION,
            started_at: self.started_at,
            uptime_secs: self.started.elapsed().as_secs(),
            jobs: u64::try_from(self.jobs.in_progress()).unwrap_or(u64::MAX),
            socket: self.socket.clone(),
        }
    }

    async fn stop(&self) {
        tracing::info!("stop requested");
        self.shutdown.cancel();
    }
}
