//! What the rho daemon answers over `rho-protocol`.
//!
//! `crate::daemon` is the generic transport (socket, lock, accept loop) and
//! never learns what a session is; [`DaemonHost`] is the [`Service`] it routes
//! requests to, and `app::daemon_cli` wires the two together. The host owns
//! the job registry (`jobs/`), whose records outlive the daemon, and the
//! sessions it has loaded (`sessions/`), which own the budget their
//! subagents share (`subagents.rs`). It also writes the credential store
//! those sessions read (`auth.rs`), reports and chooses that store
//! (`store.rs`), and runs the interactive logins clients start, relaying
//! each to its client (`logins.rs`).

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use rho_protocol::{
    AuthChooseStoreParams, AuthChooseStoreResponse, AuthKeyParams, AuthKeyResponse,
    AuthListResponse, AuthLoginCancelParams, AuthLoginCancelResponse, AuthLoginParams,
    AuthLoginResponse, AuthLogoutParams, AuthLogoutResponse, ConfigGetParams, ConfigRegisterParams,
    ConfigRegisterResponse, ConfigSchema, ConfigSetParams, ConfigSetResponse, ConfigWatch,
    DaemonStatus, JobListResponse, LoginStream, RpcError, Service, SessionAnswerParams,
    SessionAnswerResponse, SessionAttachParams, SessionAttachResponse, SessionCancelParams,
    SessionCancelResponse, SessionConfigureParams, SessionConfigureResponse, SessionDeleteParams,
    SessionDeleteResponse, SessionHoldParams, SessionHoldResponse, SessionListParams,
    SessionListResponse, SessionNewParams, SessionNewResponse, SessionObserveParams,
    SessionPromptParams, SessionPromptResponse, SessionRenameParams, SessionRenameResponse,
    SessionSteerParams, SessionSteerResponse, SessionUnsteerParams, SessionUnsteerResponse,
    Subscription, PROTOCOL_VERSION,
};
use rho_providers::credentials::CredentialStoreBackend;
use rho_sdk::CancellationToken;

use crate::daemon::DaemonLock;

mod auth;
mod jobs;
mod logins;
mod sessions;
mod settings;
mod store;
mod subagents;
mod wire;

use jobs::{JobRegistry, JobStore};
use logins::Logins;
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
    /// Shared with the logins, which refresh the sessions' options once
    /// they saved a credential.
    sessions: Arc<Sessions>,
    logins: Arc<Logins>,
    /// `RHO_CREDENTIAL_STORE` in the daemon's environment, parsed once at
    /// startup, which refuses a value it does not know.
    store_environment: Option<CredentialStoreBackend>,
}

impl DaemonHost {
    /// A host for the daemon of `rho_home` listening on `socket` since
    /// `started_at` (Unix seconds), keeping its jobs in `jobs`, with
    /// `store_environment` its parsed `RHO_CREDENTIAL_STORE`.
    /// `rho/daemon/stop` calls [`Self::begin_stop`], which cancels
    /// `shutdown`, the transport's; once the transport stopped serving, call
    /// [`Self::stop_sessions`].
    pub(super) fn new(
        JobRecords(jobs): JobRecords,
        rho_home: &Path,
        socket: PathBuf,
        started_at: u64,
        shutdown: CancellationToken,
        store_environment: Option<CredentialStoreBackend>,
    ) -> Self {
        Self {
            socket,
            started_at,
            started: Instant::now(),
            sessions: Arc::new(Sessions::new(Arc::clone(&jobs), rho_home)),
            logins: Arc::new(Logins::new()),
            shutdown,
            jobs,
            store_environment,
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

    /// Unloads each session once it sat idle for `grace`, or discards it
    /// when it saved nothing, until the daemon starts stopping (#1661).
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
            credential_store: store::status(self.store_environment).await,
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

    async fn unsteer_session(
        &self,
        params: SessionUnsteerParams,
    ) -> Result<SessionUnsteerResponse, RpcError> {
        self.sessions.unsteer(params).await
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

    async fn session_mcp(
        &self,
        params: rho_protocol::SessionMcpParams,
    ) -> Result<rho_protocol::SessionMcpInventory, RpcError> {
        self.sessions.mcp(params).await
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

    async fn observe_session(
        &self,
        params: SessionObserveParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        self.sessions.observe(params).await
    }

    async fn list_sessions(
        &self,
        params: SessionListParams,
    ) -> Result<SessionListResponse, RpcError> {
        self.sessions.list(params).await
    }

    async fn rename_session(
        &self,
        params: SessionRenameParams,
    ) -> Result<SessionRenameResponse, RpcError> {
        self.sessions.rename(params).await
    }

    async fn delete_session(
        &self,
        params: SessionDeleteParams,
    ) -> Result<SessionDeleteResponse, RpcError> {
        self.sessions.delete(params).await
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
        auth::credentials_changed(&self.sessions).await;
        Ok(saved)
    }

    async fn logout_auth(&self, params: AuthLogoutParams) -> Result<AuthLogoutResponse, RpcError> {
        let logged_out = auth::logout(params).await?;
        auth::credentials_changed(&self.sessions).await;
        Ok(logged_out)
    }

    async fn list_auths(&self) -> Result<AuthListResponse, RpcError> {
        auth::list().await
    }

    async fn login(
        &self,
        params: AuthLoginParams,
    ) -> Result<(AuthLoginResponse, LoginStream), RpcError> {
        self.logins.start(params, Arc::clone(&self.sessions)).await
    }

    async fn cancel_login(
        &self,
        params: AuthLoginCancelParams,
    ) -> Result<AuthLoginCancelResponse, RpcError> {
        self.logins.cancel(params)
    }

    async fn choose_store(
        &self,
        params: AuthChooseStoreParams,
    ) -> Result<AuthChooseStoreResponse, RpcError> {
        store::choose(params, self.store_environment, &self.sessions).await
    }

    async fn config_schema(&self) -> Result<ConfigSchema, RpcError> {
        Ok(self.sessions.settings().schema())
    }

    async fn register_config(
        &self,
        params: ConfigRegisterParams,
    ) -> Result<ConfigRegisterResponse, RpcError> {
        self.sessions.settings().register(params).await
    }

    async fn get_config(&self, params: ConfigGetParams) -> Result<ConfigWatch, RpcError> {
        self.sessions.settings().get(params).await
    }

    async fn set_config(&self, params: ConfigSetParams) -> Result<ConfigSetResponse, RpcError> {
        self.sessions.settings().set(params).await
    }
}
