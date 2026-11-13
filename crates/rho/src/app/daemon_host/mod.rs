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
//! each to its client (`logins.rs`). It re-executes the daemon in place on
//! `rho/daemon/restart` and once a replaced executable finds it idle
//! (`upgrade.rs`, #1713).

use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use rho_protocol::{
    AuthChooseStoreParams, AuthChooseStoreResponse, AuthKeyParams, AuthKeyResponse, AuthListParams,
    AuthListResponse, AuthLoginCancelParams, AuthLoginCancelResponse, AuthLoginParams,
    AuthLoginResponse, AuthLogoutParams, AuthLogoutResponse, BuildId, ClientInfo, ConfigGetParams,
    ConfigRegisterParams, ConfigRegisterResponse, ConfigSchema, ConfigSetParams, ConfigSetResponse,
    ConfigWatch, DaemonStatus, DetachReason, JobListResponse, LoginStream, PendingUpgrade,
    ProtocolVersion, RestartCost, RpcError, Service, SessionAnswerParams, SessionAnswerResponse,
    SessionAttachParams, SessionAttachResponse, SessionCancelParams, SessionCancelResponse,
    SessionConfigureParams, SessionConfigureResponse, SessionDeleteParams, SessionDeleteResponse,
    SessionHoldParams, SessionHoldResponse, SessionListParams, SessionListResponse,
    SessionNewParams, SessionNewResponse, SessionObserveParams, SessionPromptParams,
    SessionPromptResponse, SessionRenameParams, SessionRenameResponse, SessionSteerParams,
    SessionSteerResponse, SessionUnsteerParams, SessionUnsteerResponse, SettingsService,
    Subscription, WorktreePruneParams, WorktreePruneResponse, PROTOCOL_VERSION,
};
use rho_providers::credentials::CredentialStoreBackend;

use crate::daemon::{DaemonLock, Ending, Executable, Shutdown};

mod auth;
mod jobs;
mod logins;
mod sessions;
mod settings;
mod store;
mod subagents;
mod upgrade;
mod wire;

use jobs::{JobRegistry, JobStore, JobSubject};
use logins::Logins;
use sessions::{worktree_stale, Prune, Sessions};
pub(super) use sessions::{DAEMON_COMMANDS, UNLOAD_IDLE_AFTER};
use upgrade::UPGRADE_CHECK_INTERVAL;
use upgrade::{Check, Upgrades, Watch};

/// How often the daemon prunes session worktrees, after the pass it runs
/// at start.
//
// Policy: once a day, as the cleanup proposal Matt approved in #1917 says.
// A pass runs git on each kept worktree, so it does not run per listing.
const WORKTREE_PRUNE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after the daemon starts its first worktree pass runs.
//
// Tripwire, not measured to the millisecond: a client that starts the
// daemon, as `rho worktrees prune` can, sends its request well under a
// second after the daemon answers (each e2e prune test runs in about 1 s
// whole), so its own pass removes, and reports, what the daemon's would
// have. The daemon's pass skips sessions used within the last day, so
// waiting a minute delays nothing it would remove.
const WORKTREE_PRUNE_AFTER_START: Duration = Duration::from_secs(60);

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
    shutdown: Shutdown,
    jobs: Arc<JobRegistry>,
    /// Shared with the logins, which refresh the sessions' options once
    /// they saved a credential.
    sessions: Arc<Sessions>,
    logins: Arc<Logins>,
    upgrades: Upgrades,
    /// Connections whose handshake the daemon accepted and that have not
    /// left: the clients a restart would disconnect.
    clients: AtomicU64,
    /// `RHO_CREDENTIAL_STORE` in the daemon's environment, parsed once at
    /// startup, which refuses a value it does not know.
    store_environment: Option<CredentialStoreBackend>,
}

impl DaemonHost {
    /// A host for the daemon of `rho_home` listening on `socket` since
    /// `started_at` (Unix seconds), keeping its jobs in `jobs`, with
    /// `store_environment` its parsed `RHO_CREDENTIAL_STORE`.
    /// `rho/daemon/stop` calls [`Self::begin_stop`], and `rho/daemon/restart`
    /// [`Self::begin_restart`], which request `shutdown`, the transport's;
    /// once the transport stopped serving, call [`Self::stop_sessions`].
    pub(super) fn new(
        JobRecords(jobs): JobRecords,
        rho_home: &Path,
        socket: PathBuf,
        started_at: u64,
        shutdown: Shutdown,
        store_environment: Option<CredentialStoreBackend>,
    ) -> Self {
        Self {
            socket,
            started_at,
            started: Instant::now(),
            sessions: Sessions::new(Arc::clone(&jobs), rho_home),
            logins: Arc::new(Logins::new()),
            upgrades: Upgrades::default(),
            clients: AtomicU64::new(0),
            shutdown,
            jobs,
            store_environment,
        }
    }

    /// Starts the daemon's stop: refuses new sessions and prompts, detaches
    /// every attached client as `daemon_stopping`, then requests the
    /// transport's `shutdown`, which closes the connections. How every stop
    /// begins, from `rho/daemon/stop` or a signal.
    pub(super) fn begin_stop(&self) {
        self.begin(Ending::Stop);
    }

    /// Starts the daemon's restart, as [`Self::begin_stop`] starts its stop
    /// but telling clients `restarting`; the caller re-executes the daemon
    /// once its turns are stopped.
    pub(super) fn begin_restart(&self) {
        self.begin(Ending::Restart);
    }

    fn begin(&self, ending: Ending) {
        self.shutdown.request(ending, |ending| {
            self.sessions.begin_stop(match ending {
                Ending::Stop => DetachReason::DaemonStopping,
                Ending::Restart => DetachReason::Restarting,
            });
        });
    }

    /// Claims the daemon idle for an upgrade, unless anything would be cut
    /// short; once claimed, the restart follows at once. Closing the job
    /// registry is the claim: no turn starts after it. It closes only if no
    /// job went in progress since the daemon was found idle, before the
    /// other checks, so a turn that began and ended meanwhile cannot leave
    /// work those checks missed. Background work and subagents both start in
    /// a turn; only a login or a settings change sent between the checks and
    /// the close escapes them, and the restart then ends it as a stop would.
    fn claim_idle(&self) -> bool {
        let Some(idle) = self.jobs.idle() else {
            return false;
        };
        self.work_outside_jobs() == 0 && self.jobs.close_if_still(idle)
    }

    /// What a restart would cut short besides jobs in progress: logins
    /// under way and sessions with work of their own. An upgrade waits for
    /// it, and a newer client is told of it, so both count it here.
    fn work_outside_jobs(&self) -> usize {
        self.logins.under_way() + self.sessions.upgrade_blockers()
    }

    /// Everything a restart would cut short, which an upgrade waits for.
    fn in_progress(&self) -> u64 {
        u64::try_from(self.jobs.in_progress() + self.work_outside_jobs()).unwrap_or(u64::MAX)
    }

    /// What a restart would cost now: what an upgrade waits for, which
    /// `status` reports pending, and the clients it would leave without a
    /// daemon they can use.
    fn restart_cost(&self) -> RestartCost {
        RestartCost {
            in_progress: self.in_progress(),
            clients: self.clients.load(Ordering::Relaxed),
        }
    }

    /// Whether the daemon's stop, or restart, has begun.
    pub(super) fn stopping(&self) -> bool {
        self.shutdown.is_requested()
    }

    /// Restarts the daemon once `executable` was replaced and nothing would
    /// be cut short, checking every [`UPGRADE_CHECK_INTERVAL`]; ends once
    /// the daemon stops.
    pub(super) async fn upgrade_when_replaced(&self, executable: &Executable) {
        let mut checks = tokio::time::interval(UPGRADE_CHECK_INTERVAL);
        checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut watch = Watch::default();
        loop {
            tokio::select! {
                () = self.upgrades.next_check(&mut checks) => {}
                () = self.shutdown.requested() => return,
            }
            if self.upgrades.check(executable, &mut watch).await == Check::Upgrade
                && self.claim_idle()
            {
                tracing::info!("upgrading in place");
                self.begin_restart();
                return;
            }
        }
    }

    /// Unloads each session once it sat idle for `grace`, or discards it
    /// when it saved nothing, until the daemon starts stopping (#1661).
    pub(super) async fn unload_idle_sessions(&self, grace: Duration) {
        self.sessions.unload_idle(grace).await;
    }

    /// Prunes the session worktrees [`WORKTREE_PRUNE_AFTER_START`] after
    /// the daemon starts and every [`WORKTREE_PRUNE_INTERVAL`] after that,
    /// until the daemon starts stopping.
    pub(super) async fn prune_worktrees_daily(&self) {
        let mut wait = WORKTREE_PRUNE_AFTER_START;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = self.shutdown.requested() => return,
            }
            self.sessions.prune_worktrees(Prune::Daily).await;
            wait = WORKTREE_PRUNE_INTERVAL;
        }
    }

    /// Stops the turns under way, waits for them to save, and records
    /// them interrupted; the wait's budget runs from `stop_requested`. Call
    /// once the transport stopped serving, before releasing the daemon lock.
    /// Returns whether all of it finished in time: a turn stuck past the
    /// budget still holds processes only exiting ends.
    pub(super) async fn stop_sessions(&self, stop_requested: tokio::time::Instant) -> bool {
        self.sessions.stop(stop_requested).await
    }
}

impl Service for DaemonHost {
    async fn status(&self) -> DaemonStatus {
        let in_progress = self.jobs.in_progress();
        DaemonStatus {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: PROTOCOL_VERSION,
            started_at: self.started_at,
            uptime_secs: self.started.elapsed().as_secs(),
            jobs: u64::try_from(in_progress).unwrap_or(u64::MAX),
            in_progress: self.in_progress(),
            socket: self.socket.clone(),
            credential_store: store::status(self.store_environment).await,
            upgrade: self.upgrades.pending().map(|version| PendingUpgrade {
                version,
                waiting_for: self.in_progress(),
            }),
        }
    }

    fn hello(&self, client: &ClientInfo) {
        self.clients.fetch_add(1, Ordering::Relaxed);
        self.upgrades.heard(client);
    }

    fn left(&self) {
        self.clients.fetch_sub(1, Ordering::Relaxed);
    }

    fn refused_newer(&self, client: ProtocolVersion) -> RestartCost {
        let cost = self.restart_cost();
        // That client stops this daemon when the restart is free, so the
        // log says whether it may.
        tracing::info!(
            %client,
            in_progress = cost.in_progress,
            clients = cost.clients,
            may_replace = cost.is_free(),
            "refused a client of a newer protocol"
        );
        cost
    }

    fn skewed(&self, client: &ClientInfo, build: &BuildId) -> RestartCost {
        let cost = self.restart_cost();
        tracing::info!(
            client = %client.name,
            version = %client.version,
            %build,
            in_progress = cost.in_progress,
            clients = cost.clients,
            "accepted a client of another build"
        );
        cost
    }

    async fn stop(&self) {
        tracing::info!("stop requested");
        self.begin_stop();
    }

    async fn restart(&self) {
        tracing::info!("restart requested");
        self.begin_restart();
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

    async fn session_processes(
        &self,
        params: rho_protocol::SessionProcessesParams,
    ) -> Result<rho_protocol::SessionProcessesResponse, RpcError> {
        self.sessions.processes(params).await
    }

    async fn session_background_stop(
        &self,
        params: rho_protocol::SessionBackgroundStopParams,
    ) -> Result<rho_protocol::SessionBackgroundStopResponse, RpcError> {
        self.sessions.background_stop(params).await
    }

    async fn session_mcp(
        &self,
        params: rho_protocol::SessionMcpParams,
    ) -> Result<rho_protocol::SessionMcpInventory, RpcError> {
        self.sessions.mcp(params).await
    }

    async fn session_mcp_catalog(
        &self,
        params: rho_protocol::SessionMcpCatalogParams,
    ) -> Result<rho_protocol::SessionMcpCatalog, RpcError> {
        self.sessions.mcp_catalog(params).await
    }

    async fn session_mcp_resource(
        &self,
        params: rho_protocol::SessionMcpResourceParams,
    ) -> Result<rho_protocol::SessionMcpResourceResponse, RpcError> {
        self.sessions.mcp_resource(params).await
    }

    async fn session_mcp_complete(
        &self,
        params: rho_protocol::SessionMcpCompleteParams,
    ) -> Result<rho_protocol::SessionMcpCompleteResponse, RpcError> {
        self.sessions.mcp_complete(params).await
    }

    async fn inspect_session(
        &self,
        params: rho_protocol::SessionInspectParams,
    ) -> Result<rho_protocol::SessionInspectResponse, RpcError> {
        self.sessions.inspect(params).await
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
        // A drive marks its run finished before its job ends, so a job read
        // first that has ended is already in the tracker read second.
        let records = self.jobs.list();
        let undelivered = self.sessions.undelivered_workflow_runs();
        let in_use = self.sessions.worktrees_in_use(&records);
        let now = jobs::unix_now();
        JobListResponse {
            jobs: records
                .into_iter()
                .map(|record| {
                    let queue = self.sessions.subagents().queue_place(&record.id);
                    let live = match &record.subject {
                        JobSubject::Session { session } => self.sessions.session_live(session),
                        // Only the run under way: an ended run of the same
                        // session, as a workflow node that ran again has,
                        // is followed no more.
                        JobSubject::Subagent {
                            session: Some(session),
                            run: _,
                        } if record.state.in_progress() => {
                            self.sessions.subagents().feeds().live(session)
                        }
                        JobSubject::Subagent { .. } | JobSubject::Workflow { .. } => None,
                    };
                    let worktree_stale = worktree_stale(&record, &in_use, now);
                    wire::job_summary(record, queue, &undelivered, live, worktree_stale)
                })
                .collect(),
        }
    }

    async fn prune_worktrees(
        &self,
        params: WorktreePruneParams,
    ) -> Result<WorktreePruneResponse, RpcError> {
        let WorktreePruneParams { dry_run } = params;
        let prune = match dry_run {
            true => Prune::DryRun,
            false => Prune::Now,
        };
        Ok(WorktreePruneResponse {
            worktrees: self.sessions.prune_worktrees(prune).await,
        })
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

    async fn list_auths(&self, params: AuthListParams) -> Result<AuthListResponse, RpcError> {
        auth::list(params).await
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
}

impl SettingsService for DaemonHost {
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
