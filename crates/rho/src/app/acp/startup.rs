//! Keep login discovery independent of model credentials and network discovery.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use agent_client_protocol::{
    schema::v1::{DeleteSessionRequest, DeleteSessionResponse},
    Error as AcpError,
};
use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};

use super::agent::RhoAcpAgent;
use crate::{
    app::{bootstrap, config_repository::ConfigRepository},
    cli::Cli,
};

pub(super) struct LazyAgent {
    // Shared owns the suspended initializer even if one waiting request is
    // cancelled. Startup failures remain stable until the host reconnects.
    pending: Shared<BoxFuture<'static, Result<Arc<RhoAcpAgent>, Arc<str>>>>,
    // Store-only deletes must finish before ready hands out the first agent,
    // so a concurrently finishing startup cannot load a session being deleted.
    publication: tokio::sync::Mutex<()>,
    closed: AtomicBool,
}

impl LazyAgent {
    pub(super) fn new(cli: Cli) -> anyhow::Result<Self> {
        // Only config-defined auth profiles are needed for initialize. Do not
        // resolve models, refresh catalogs, or bind an agent until a session.
        ConfigRepository::new(cli.config.clone())
            .load()?
            .providers
            .activate()?;
        let pending = async move {
            bootstrap::prepare_acp(cli)
                .await
                .map(|startup| Arc::new(RhoAcpAgent::new(startup)))
                .map_err(|error| Arc::<str>::from(format!("{error:#}")))
        }
        .boxed()
        .shared();
        Ok(Self {
            pending,
            publication: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
        })
    }

    pub(super) async fn ready(&self) -> Result<Arc<RhoAcpAgent>, AcpError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(AcpError::invalid_request().data("ACP connection is closed"));
        }
        let agent = self
            .pending
            .clone()
            .await
            .map_err(|error| AcpError::internal_error().data(error.to_string()))?;
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
            let agent = Arc::clone(agent);
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

    pub(super) fn started(&self) -> Option<&Arc<RhoAcpAgent>> {
        self.pending.peek().and_then(|result| result.as_ref().ok())
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
