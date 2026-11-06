//! What the rho daemon answers over `rho-protocol`.
//!
//! `crate::daemon` is the generic transport (socket, lock, accept loop) and
//! never learns what a session is; [`DaemonHost`] is the [`Service`] it routes
//! requests to, and `app::daemon_cli` wires the two together. The host owns
//! the job registry (`jobs/`), whose records outlive the daemon, and the
//! sessions it has loaded (`sessions/`).

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use rho_protocol::{
    DaemonStatus, JobsListResponse, RpcError, Service, SessionCancelParams, SessionCancelResponse,
    SessionNewParams, SessionNewResponse, SessionPromptParams, SessionPromptResponse,
    SessionSteerParams, SessionSteerResponse, PROTOCOL_VERSION,
};
use rho_sdk::CancellationToken;

use crate::daemon::DaemonLock;

mod jobs;
mod sessions;
mod wire;

use jobs::{JobRegistry, JobStore};
use sessions::Sessions;

/// The daemon's job records, opened before the daemon binds its socket, so
/// a daemon that cannot keep records stops before any client reaches it.
pub(super) struct JobRecords(Arc<JobRegistry>);

impl JobRecords {
    /// Opens the records in the daemon's `state_dir`. Opening rewrites the
    /// records of turns an earlier daemon left open, so it takes the daemon
    /// lock, which guards that directory, as proof the caller holds it.
    pub(super) fn open(_held: &DaemonLock, state_dir: &Path) -> io::Result<Self> {
        let store = JobStore::open(state_dir)?;
        Ok(Self(Arc::new(JobRegistry::open(store))))
    }
}

/// The daemon's protocol service.
pub(super) struct DaemonHost {
    socket: PathBuf,
    started_at: u64,
    started: Instant,
    shutdown: CancellationToken,
    jobs: Arc<JobRegistry>,
    sessions: Sessions,
}

impl DaemonHost {
    /// A host for the daemon listening on `socket` since `started_at` (Unix
    /// seconds), keeping its jobs in `jobs`. `rho/daemon/stop` cancels
    /// `shutdown`, which ends the transport's serve loop and refuses new
    /// sessions and prompts; then call [`Self::stop_sessions`].
    pub(super) fn new(
        JobRecords(jobs): JobRecords,
        socket: PathBuf,
        started_at: u64,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            socket,
            started_at,
            started: Instant::now(),
            sessions: Sessions::new(Arc::clone(&jobs), shutdown.clone()),
            shutdown,
            jobs,
        }
    }

    /// Cancels the turns under way, waits for them to save, and records
    /// them interrupted; the wait's budget runs from `stop_requested`. Call
    /// once the transport stopped serving, before releasing the daemon lock.
    pub(super) async fn stop_sessions(&self, stop_requested: tokio::time::Instant) {
        self.sessions.stop(stop_requested).await;
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

    async fn new_session(&self, params: SessionNewParams) -> Result<SessionNewResponse, RpcError> {
        self.sessions.open(params).await
    }

    async fn prompt_session(
        &self,
        params: SessionPromptParams,
    ) -> Result<SessionPromptResponse, RpcError> {
        self.sessions.prompt(params).await
    }

    async fn cancel_session(
        &self,
        params: SessionCancelParams,
    ) -> Result<SessionCancelResponse, RpcError> {
        self.sessions.cancel(params).await
    }

    async fn steer_session(
        &self,
        params: SessionSteerParams,
    ) -> Result<SessionSteerResponse, RpcError> {
        self.sessions.steer(params).await
    }

    async fn list_jobs(&self) -> JobsListResponse {
        JobsListResponse {
            jobs: self.jobs.list().into_iter().map(Into::into).collect(),
        }
    }
}
