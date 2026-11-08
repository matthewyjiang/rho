//! What the rho daemon answers over `rho-protocol`.
//!
//! `crate::daemon` is the generic transport (socket, lock, accept loop) and
//! never learns what a session is; [`DaemonHost`] is the [`Service`] it routes
//! requests to, and `app::daemon_cli` wires the two together. The host owns
//! the job registry (`jobs/`), whose records outlive the daemon, and the
//! sessions it has loaded (`sessions/`), which own the budget their
//! subagents share (`subagents.rs`). It also writes the credential store
//! those sessions read (`auth.rs`).

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use rho_protocol::{
    AuthKeyParams, AuthKeyResponse, AuthLogoutParams, AuthLogoutResponse, DaemonStatus,
    JobListResponse, RpcError, Service, SessionAnswerParams, SessionAnswerResponse,
    SessionAttachParams, SessionAttachResponse, SessionCancelParams, SessionCancelResponse,
    SessionConfigureParams, SessionConfigureResponse, SessionHoldParams, SessionHoldResponse,
    SessionNewParams, SessionNewResponse, SessionPromptParams, SessionPromptResponse,
    SessionSteerParams, SessionSteerResponse, Subscription, PROTOCOL_VERSION,
};
use rho_sdk::CancellationToken;

use crate::daemon::DaemonLock;

mod auth;
mod jobs;
mod sessions;
mod subagents;
mod wire;

use jobs::{JobRegistry, JobStore};
use sessions::Sessions;
pub(super) use sessions::UNLOAD_IDLE_AFTER;

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
    /// seconds), keeping its jobs in `jobs`. `rho/daemon/stop` calls
    /// [`Self::begin_stop`], which cancels `shutdown`, the transport's; once
    /// the transport stopped serving, call [`Self::stop_sessions`].
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
            sessions: Sessions::new(Arc::clone(&jobs)),
            shutdown,
            jobs,
        }
    }

    /// Starts the daemon's stop: refuses new sessions and prompts, detaches
    /// every attached client as `daemon_stopping`, then cancels the
    /// transport's `shutdown`, which closes the connections. How every stop
    /// begins, from `rho/daemon/stop` or a signal.
    pub(super) fn begin_stop(&self) {
        self.sessions.begin_stop();
        self.shutdown.cancel();
    }

    /// Whether the daemon's stop has begun.
    pub(super) fn stopping(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// Unloads each session once it sat idle for `grace`, until the daemon
    /// starts stopping (#1661).
    pub(super) async fn unload_idle_sessions(&self, grace: Duration) {
        self.sessions.unload_idle(grace).await;
    }

    /// Stops the turns under way, waits for them to save, and records
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
        self.begin_stop();
    }

    async fn new_session(&self, params: SessionNewParams) -> Result<SessionNewResponse, RpcError> {
        self.sessions.open_new(params).await
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

    async fn answer_session(
        &self,
        params: SessionAnswerParams,
    ) -> Result<SessionAnswerResponse, RpcError> {
        self.sessions.answer(params).await
    }

    async fn hold_session(
        &self,
        params: SessionHoldParams,
    ) -> Result<SessionHoldResponse, RpcError> {
        self.sessions.hold(params).await
    }

    async fn configure_session(
        &self,
        params: SessionConfigureParams,
    ) -> Result<SessionConfigureResponse, RpcError> {
        self.sessions.configure(params).await
    }

    async fn attach_session(
        &self,
        params: SessionAttachParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        self.sessions.attach(params).await
    }

    async fn list_jobs(&self) -> JobListResponse {
        JobListResponse {
            jobs: self
                .jobs
                .list()
                .into_iter()
                .map(|record| {
                    let queue = self.sessions.subagents().queue_place(&record.id);
                    wire::job_summary(record, queue)
                })
                .collect(),
        }
    }

    // A credential write changes the models every session may switch to.
    async fn save_auth_key(&self, params: AuthKeyParams) -> Result<AuthKeyResponse, RpcError> {
        let saved = auth::save_key(params).await?;
        self.sessions.refresh_all_options().await;
        Ok(saved)
    }

    async fn logout_auth(&self, params: AuthLogoutParams) -> Result<AuthLogoutResponse, RpcError> {
        let logged_out = auth::logout(params).await?;
        self.sessions.refresh_all_options().await;
        Ok(logged_out)
    }
}
