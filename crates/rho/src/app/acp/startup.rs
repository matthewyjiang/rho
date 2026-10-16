//! Keep login discovery independent of model credentials and network discovery.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use agent_client_protocol::Error as AcpError;
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
        // Close must not start a cold initializer, but an initializer already
        // in flight must not publish a live session after the connection closes.
        if self.closed.load(Ordering::Acquire) {
            agent.shutdown_all().await;
            return Err(AcpError::invalid_request().data("ACP connection is closed"));
        }
        Ok(agent)
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
