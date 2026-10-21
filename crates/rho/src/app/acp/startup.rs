//! Keep login discovery independent of model credentials and network discovery.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, PoisonError,
};

use agent_client_protocol::{
    schema::v1::{
        AuthenticateRequest, AuthenticateResponse, DeleteSessionRequest, DeleteSessionResponse,
        InitializeRequest, LogoutResponse, RequestId,
    },
    Error as AcpError, ErrorCode,
};
use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};

use super::{
    agent::RhoAcpAgent,
    auth::{self, CredentialFailure, CredentialStoreChoice, OAuthSignIn},
    auth_login::LoginFlight,
    host_features::NegotiatedFeatures,
    AcpClientPort,
};
use crate::{
    app::{bootstrap, config_repository::ConfigRepository},
    cli::Cli,
    credential_store::AppCredentialStore,
};

/// Builds the agent, or answers the error every waiting request gets.
type StartupRun = BoxFuture<'static, Result<Arc<RhoAcpAgent>, Box<StartupFailure>>>;

/// A failed startup run: the error waiting requests get, and the credential
/// failure behind it, which `logout` needs and the error no longer says.
/// Boxed where returned, since it outgrows Clippy's `Result` error budget.
#[derive(Clone, Debug)]
struct StartupFailure {
    error: AcpError,
    credentials: Option<CredentialFailure>,
}

impl StartupFailure {
    fn new(error: &anyhow::Error) -> Box<Self> {
        Box::new(Self {
            error: auth::request_error(error, || format!("{error:#}")),
            credentials: CredentialFailure::find(error),
        })
    }

    /// Whether startup failed because nothing is stored for the selected
    /// auth, which leaves nothing to sign out of.
    fn found_no_credentials(&self) -> bool {
        matches!(self.credentials, Some(CredentialFailure::Missing(_)))
    }
}

impl From<AcpError> for Box<StartupFailure> {
    fn from(error: AcpError) -> Self {
        Box::new(StartupFailure {
            error,
            credentials: None,
        })
    }
}

/// One run of deferred startup. Shared owns the suspended initializer even if
/// one waiting request is cancelled.
type Startup = Shared<StartupRun>;

pub(super) struct LazyAgent {
    // Startup failures remain stable until the host reconnects, except
    // `auth_required`: the host's sign-in runs a login (in a terminal or
    // through `authenticate`) and then asks again on this connection, so that
    // failure starts a fresh run.
    pending: std::sync::Mutex<Startup>,
    start: Box<dyn Fn() -> StartupRun + Send + Sync>,
    // Store-only deletes must finish before ready hands out the first agent,
    // so a concurrently finishing startup cannot load a session being deleted.
    publication: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    host_features: Arc<NegotiatedFeatures>,
    login: LoginSetup,
    flight: LoginFlight,
}

/// What `authenticate` and `logout` need from launch.
#[derive(Clone, Debug)]
struct LoginSetup {
    /// The `--config` override, where `authenticate` and `logout` find the
    /// store.
    config: Option<std::path::PathBuf>,
    /// Read once at launch, before `initialize` advertises sign-in.
    store: CredentialStoreChoice,
}

impl LazyAgent {
    pub(super) fn new(cli: Cli) -> anyhow::Result<Self> {
        // Only config-defined auth profiles are needed for initialize. Do not
        // resolve models, refresh catalogs, or bind an agent until a session.
        let config = ConfigRepository::new(cli.config.clone()).load()?;
        config.providers.activate()?;
        let login = LoginSetup {
            config: cli.config.clone(),
            store: if crate::credential_store::needs_explicit_choice(&config) {
                CredentialStoreChoice::Pending
            } else {
                CredentialStoreChoice::Made
            },
        };
        let host_features = Arc::new(NegotiatedFeatures::default());
        let negotiated = Arc::clone(&host_features);
        // Polled first by a session request, which follows `initialize`.
        let start = move || {
            let (cli, negotiated) = (cli.clone(), Arc::clone(&negotiated));
            async move {
                // A retry follows a terminal login, which another process saved.
                AppCredentialStore::reread_external_logins();
                bootstrap::prepare_acp(cli, negotiated.get(), negotiated.client())
                    .await
                    .map(|startup| Arc::new(RhoAcpAgent::new(startup)))
                    .map_err(|error| StartupFailure::new(&error))
            }
            .boxed()
        };
        Ok(Self::starting_with(start, host_features, login))
    }

    fn starting_with(
        start: impl Fn() -> StartupRun + Send + Sync + 'static,
        host_features: Arc<NegotiatedFeatures>,
        login: LoginSetup,
    ) -> Self {
        Self {
            pending: std::sync::Mutex::new(start().shared()),
            start: Box::new(start),
            publication: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            host_features,
            login,
            flight: LoginFlight::default(),
        }
    }

    /// Where OAuth logins run, from the features `initialize` recorded.
    pub(super) fn oauth_sign_in(&self) -> OAuthSignIn {
        auth::oauth_sign_in(self.host_features.get(), self.login.store)
    }

    /// Runs an `authenticate` login in this process, one at a time. It needs
    /// no model or session, so it does not wait for startup; the next session
    /// request after an `auth_required` failure starts a fresh run that sees
    /// the saved credentials.
    pub(super) async fn authenticate(
        &self,
        request: AuthenticateRequest,
        request_id: RequestId,
        port: &dyn AcpClientPort,
    ) -> Result<AuthenticateResponse, AcpError> {
        self.flight
            .run(super::auth_login::authenticate(
                &request.method_id.0,
                self.oauth_sign_in(),
                self.login.config.clone(),
                request_id,
                port,
            ))
            .await
    }

    /// Runs `logout` in the login flight, so it supersedes an `authenticate`
    /// still waiting, as a newer `authenticate` would, and a login cannot
    /// save credentials after the logout deleted them. The signed-out auth
    /// profile is the one new sessions use, so this waits for startup. A
    /// startup that found no credentials is signed out already; refused or
    /// unreadable ones are still stored, so that failure is returned.
    pub(super) async fn logout(&self) -> Result<LogoutResponse, AcpError> {
        // Boxed so the startup wait and the logout are not held inline in
        // the flight's future: request handlers run on the main thread,
        // whose 1 MiB Windows stack this future overflowed unboxed.
        self.flight
            .run(Box::pin(async {
                match self.started_or_failure().await {
                    Ok(agent) => agent.logout(self.login.config.clone()).await?,
                    Err(failure) if failure.found_no_credentials() => {}
                    Err(failure) => return Err(failure.error),
                }
                Ok(LogoutResponse::new())
            }))
            .await
    }

    /// Records the optional features the host advertised at `initialize`
    /// and the connection it arrived on.
    pub(super) fn record_client(
        &self,
        request: &InitializeRequest,
        client: Arc<dyn AcpClientPort>,
    ) {
        self.host_features.record(request, client);
    }

    pub(super) async fn ready(&self) -> Result<Arc<RhoAcpAgent>, AcpError> {
        self.started_or_failure()
            .await
            .map_err(|failure| failure.error)
    }

    async fn started_or_failure(&self) -> Result<Arc<RhoAcpAgent>, Box<StartupFailure>> {
        if self.closed.load(Ordering::Acquire) {
            return Err(AcpError::invalid_request()
                .data("ACP connection is closed")
                .into());
        }
        let startup = self.current();
        let agent = match startup.clone().await {
            Ok(agent) => agent,
            Err(failure) => {
                if failure.error.code == ErrorCode::AuthRequired {
                    // Concurrent waiters saw the same failure; only the first
                    // replaces it, so one sign-in starts one new run.
                    let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                    if pending.ptr_eq(&startup) {
                        *pending = (self.start)().shared();
                    }
                }
                return Err(failure);
            }
        };
        let _publication = self.publication.lock().await;
        // Close must not start a cold initializer, but an initializer already
        // in flight must not publish a live session after the connection closes.
        if self.closed.load(Ordering::Acquire) {
            agent.shutdown_all().await;
            return Err(AcpError::invalid_request()
                .data("ACP connection is closed")
                .into());
        }
        Ok(agent)
    }

    pub(super) async fn delete_session(
        &self,
        request: DeleteSessionRequest,
    ) -> Result<DeleteSessionResponse, AcpError> {
        let publication = self.publication.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(AcpError::invalid_request().data("ACP connection is closed"));
        }
        if let Some(agent) = self.started() {
            // The live agent owns per-session retirement/install locking.
            // Do not serialize unrelated live requests behind a deletion.
            drop(publication);
            agent.delete_session(request).await
        } else {
            // No session can be live before ready returns, even if discovery
            // is in flight or failed. History needs only the store here.
            super::session_list::delete(request.session_id).await
        }
    }

    pub(super) fn started(&self) -> Option<Arc<RhoAcpAgent>> {
        self.current()
            .peek()
            .and_then(|result| result.as_ref().ok().cloned())
    }

    fn current(&self) -> Startup {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(agent) = self.started() {
            agent.shutdown_all().await;
        }
    }
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
