//! What the rho daemon answers over `rho-protocol`.
//!
//! `crate::daemon` is the generic transport (socket, lock, accept loop) and
//! never learns what a session is; [`DaemonHost`] is the [`Service`] it routes
//! requests to, and `app::daemon_cli` wires the two together. The host owns
//! the job registry (`jobs/`), whose records outlive the daemon, and the
//! threads it runs (`threads/`).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use rho_protocol::{
    DaemonStatus, JobsListResponse, RpcError, Service, ThreadCancelParams, ThreadCancelResponse,
    ThreadCreateParams, ThreadCreateResponse, ThreadPromptParams, ThreadPromptResponse,
    ThreadSteerParams, ThreadSteerResponse, PROTOCOL_VERSION,
};
use rho_sdk::CancellationToken;

mod jobs;
mod threads;
mod wire;

use jobs::{JobRegistry, JobStore};
use threads::Threads;

/// The daemon's protocol service.
pub(super) struct DaemonHost {
    socket: PathBuf,
    started_at: u64,
    started: Instant,
    shutdown: CancellationToken,
    jobs: Arc<JobRegistry>,
    threads: Threads,
}

impl DaemonHost {
    /// A host for the daemon listening on `socket` since `started_at` (Unix
    /// seconds), keeping its job records in `rho_home`. `rho/daemon/stop`
    /// cancels `shutdown`, which ends the transport's serve loop and
    /// refuses new threads and prompts; then call [`Self::stop_threads`].
    ///
    /// Opening the registry rewrites the records of turns an earlier daemon
    /// left open, so call this only while holding the daemon lock.
    pub(super) fn new(
        socket: PathBuf,
        started_at: u64,
        shutdown: CancellationToken,
        rho_home: &Path,
    ) -> Self {
        threads::open_credential_store();
        let jobs = Arc::new(JobRegistry::open(JobStore::in_rho_home(rho_home)));
        Self {
            socket,
            started_at,
            started: Instant::now(),
            threads: Threads::new(Arc::clone(&jobs), shutdown.clone()),
            shutdown,
            jobs,
        }
    }

    /// Cancels the turns under way, waits for them to save, and records
    /// them interrupted. Call once the transport stopped serving, before
    /// releasing the daemon lock.
    pub(super) async fn stop_threads(&self) {
        self.threads.stop().await;
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

    async fn create_thread(
        &self,
        params: ThreadCreateParams,
    ) -> Result<ThreadCreateResponse, RpcError> {
        self.threads.create(params).await
    }

    async fn prompt_thread(
        &self,
        params: ThreadPromptParams,
    ) -> Result<ThreadPromptResponse, RpcError> {
        self.threads.prompt(params).await
    }

    async fn cancel_thread(
        &self,
        params: ThreadCancelParams,
    ) -> Result<ThreadCancelResponse, RpcError> {
        self.threads.cancel(params)
    }

    async fn steer_thread(
        &self,
        params: ThreadSteerParams,
    ) -> Result<ThreadSteerResponse, RpcError> {
        self.threads.steer(params)
    }

    async fn list_jobs(&self) -> JobsListResponse {
        JobsListResponse {
            jobs: self.jobs.list().into_iter().map(Into::into).collect(),
        }
    }
}
