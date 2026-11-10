//! A daemon session's options on the wire: its settings and their choices,
//! and the commands its prompts run, which the ACP adapter (#1670) and the
//! TUI client (#1667) render. [`crate::app::session_options`] builds them
//! with the daemon's commands; this keeps each session's feed current.

use std::sync::Arc;

use rho_protocol as wire;
use rho_providers::credentials::{available_auth_modes, CredentialStore};

use super::{turn::DAEMON_COMMANDS, LoadedSession, Sessions};
use crate::{
    app::{session_options, session_runtime::SessionRuntime},
    config::Config,
};

impl LoadedSession {
    /// Rebuilds the session's options from `runtime` and `auths` (see
    /// [`available_auths`]), tells attached clients when they changed, and
    /// returns them. Call after anything that may change them: a configure,
    /// a plan approval's mode switch, a credential write, or a turn, which
    /// may leave new skills, prompts, or MCP prompts behind.
    pub(super) fn refresh_options(
        &self,
        runtime: &SessionRuntime,
        auths: &[String],
    ) -> wire::SessionOptions {
        *self
            .mcp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            crate::app::session_mcp::SessionMcp::of(runtime);
        let options = daemon_options(runtime, &self.config, auths);
        self.feed.update_options(options.clone());
        self.feed.options()
    }
}

impl Sessions {
    /// Refreshes every loaded session's options after a credential write
    /// changed the models they may switch to. Each refresh waits in its own
    /// task for the session's runtime, so a session in a turn or a
    /// configure refreshes once it lets the runtime go, even when its own
    /// refresh there read the auths before the write. The runtime's lock
    /// is fair, so refreshes for later writes queue after this one's.
    pub(in crate::app::daemon_host) async fn refresh_all_options(&self) {
        let sessions: Vec<Arc<LoadedSession>> = self.loaded().values().cloned().collect();
        for session in sessions {
            tokio::spawn(async move {
                // `None` once the daemon's stop or the idle unload took it.
                if let Some(runtime) = session.runtime.lock().await.as_ref() {
                    // Each session's own: their clients' credentials differ.
                    let auths = available_auths(runtime.credentials()).await;
                    session.refresh_options(runtime, &auths);
                }
            });
        }
    }
}

/// The auths `credentials`, a session's, can use, read off the async
/// workers: a locked OS keyring blocks its reader, as `daemon_host/auth.rs`
/// notes.
pub(super) async fn available_auths(credentials: Arc<dyn CredentialStore>) -> Vec<String> {
    match tokio::task::spawn_blocking(move || available_auth_modes(credentials.as_ref())).await {
        Ok(auths) => auths,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        // Cancelled before it ran: the runtime is shutting down, and no
        // client reads options again.
        Err(_) => Vec::new(),
    }
}

/// `runtime`'s options now, with the choices `process_config` and `auths`
/// give it.
pub(super) fn daemon_options(
    runtime: &SessionRuntime,
    process_config: &Config,
    auths: &[String],
) -> wire::SessionOptions {
    session_options::of(runtime, process_config, auths, DAEMON_COMMANDS)
}
