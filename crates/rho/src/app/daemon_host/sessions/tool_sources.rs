//! Attaches a session's deferred tool sources once they load, so its MCP
//! inventory and options stop reading `connecting` without waiting for a
//! prompt. A prompt that starts first attaches them itself (see
//! `SessionRuntime::begin_prompt`) and tells the session the same way.

use std::{future::Future, sync::Arc};

use super::{options::available_auths, LoadedSession};
use crate::app::session_runtime::SessionRuntime;

impl LoadedSession {
    /// After `runtime`'s tool list changed as its tool sources attached:
    /// events map MCP calls with their servers' tools from now on, and
    /// clients see the inventory and options the tools brought.
    pub(super) async fn tool_sources_attached(&self, runtime: &SessionRuntime) {
        self.live()
            .rebind_tools(runtime.tools(), runtime.configured_context_window());
        self.refresh_options(runtime, &available_auths(runtime.credentials()).await);
    }
}

/// Waits for `loaded`, then attaches `session`'s tool sources and tells
/// its clients, unless the session was unloaded or a prompt got there first.
pub(super) fn attach_when_loaded(
    session: &Arc<LoadedSession>,
    loaded: impl Future<Output = ()> + Send + 'static,
) {
    let session = Arc::downgrade(session);
    tokio::spawn(async move {
        loaded.await;
        let Some(session) = session.upgrade() else {
            return;
        };
        let mut runtime = session.runtime.lock().await;
        let Some(runtime) = runtime.as_mut() else {
            return;
        };
        match runtime.attach_loaded_tool_sources(&session.config()).await {
            Ok(true) => session.tool_sources_attached(runtime).await,
            Ok(false) => {}
            // The tools attached, but the runtime kept its list: clients
            // still see where the servers ended up.
            Err(error) => {
                tracing::warn!(session = %session.id, "could not attach tool sources: {error:#}");
                session.tool_sources_attached(runtime).await;
            }
        }
    });
}
