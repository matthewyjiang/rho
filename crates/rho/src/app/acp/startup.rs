//! Keep login discovery independent of model credentials and network discovery.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, PoisonError,
};

use agent_client_protocol::{
    schema::v1::{ClientCapabilities, DeleteSessionRequest, DeleteSessionResponse},
    Error as AcpError, ErrorCode,
};
use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};

use super::{agent::RhoAcpAgent, auth, host_updates::NegotiatedUpdates};
use crate::{
    app::{bootstrap, config_repository::ConfigRepository},
    cli::Cli,
    credential_store::AppCredentialStore,
};

/// Builds the agent, or answers the error every waiting request gets.
type StartupRun = BoxFuture<'static, Result<Arc<RhoAcpAgent>, AcpError>>;

/// One run of deferred startup. Shared owns the suspended initializer even if
/// one waiting request is cancelled.
type Startup = Shared<StartupRun>;

pub(super) struct LazyAgent {
    // Startup failures remain stable until the host reconnects, except
    // `auth_required`: the host's sign-in runs a terminal login and then asks
    // again on this connection, so that failure starts a fresh run.
    pending: std::sync::Mutex<Startup>,
    start: Box<dyn Fn() -> StartupRun + Send + Sync>,
    // Store-only deletes must finish before ready hands out the first agent,
    // so a concurrently finishing startup cannot load a session being deleted.
    publication: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    host_updates: Arc<NegotiatedUpdates>,
}

impl LazyAgent {
    pub(super) fn new(cli: Cli) -> anyhow::Result<Self> {
        // Only config-defined auth profiles are needed for initialize. Do not
        // resolve models, refresh catalogs, or bind an agent until a session.
        ConfigRepository::new(cli.config.clone())
            .load()?
            .providers
            .activate()?;
        let host_updates = Arc::new(NegotiatedUpdates::default());
        let negotiated = Arc::clone(&host_updates);
        // Polled first by a session request, which follows `initialize`.
        let start = move || {
            let (cli, negotiated) = (cli.clone(), Arc::clone(&negotiated));
            async move {
                // A retry follows a terminal login, which another process saved.
                AppCredentialStore::reread_external_logins();
                bootstrap::prepare_acp(cli, negotiated.get())
                    .await
                    .map(|startup| Arc::new(RhoAcpAgent::new(startup)))
                    .map_err(|error| auth::request_error(&error, || format!("{error:#}")))
            }
            .boxed()
        };
        Ok(Self::starting_with(start, host_updates))
    }

    fn starting_with(
        start: impl Fn() -> StartupRun + Send + Sync + 'static,
        host_updates: Arc<NegotiatedUpdates>,
    ) -> Self {
        Self {
            pending: std::sync::Mutex::new(start().shared()),
            start: Box::new(start),
            publication: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            host_updates,
        }
    }

    /// Records the session updates the host advertised at `initialize`.
    pub(super) fn record_client(&self, capabilities: &ClientCapabilities) {
        self.host_updates.record(capabilities);
    }

    pub(super) async fn ready(&self) -> Result<Arc<RhoAcpAgent>, AcpError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(AcpError::invalid_request().data("ACP connection is closed"));
        }
        let startup = self.current();
        let agent = match startup.clone().await {
            Ok(agent) => agent,
            Err(error) => {
                if error.code == ErrorCode::AuthRequired {
                    // Concurrent waiters saw the same failure; only the first
                    // replaces it, so one sign-in starts one new run.
                    let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                    if pending.ptr_eq(&startup) {
                        *pending = (self.start)().shared();
                    }
                }
                return Err(error);
            }
        };
        let _publication = self.publication.lock().await;
        // Close must not start a cold initializer, but an initializer already
        // in flight must not publish a live session after the connection closes.
        if self.closed.load(Ordering::Acquire) {
            agent.shutdown_all().await;
            return Err(AcpError::invalid_request().data("ACP connection is closed"));
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
            let target = super::session_list::delete_target(request.session_id).await?;
            super::session_list::delete(target).await
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
