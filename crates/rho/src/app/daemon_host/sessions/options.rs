//! A daemon session's options on the wire: its settings and their choices,
//! and the commands its prompts run, which the ACP adapter (#1670) and the
//! TUI client (#1667) render. [`crate::app::session_options`] builds them
//! with the daemon's commands; this keeps each session's feed current.

use std::sync::Arc;

use rho_protocol as wire;

use super::{turn::DAEMON_COMMANDS, LoadedSession, Sessions};
pub(super) use crate::app::session_options::available_auths;
use crate::{
    app::{
        session_options,
        session_runtime::{SessionRuntime, StartedMode, StartedModel, StartedSessionDefaults},
    },
    config::Config,
};

impl LoadedSession {
    /// Rebuilds the session's options from `runtime` and `auths` (see
    /// [`available_auths`]), tells attached clients when they, the agent
    /// the session runs as, or what the sessions it starts open with
    /// changed, and returns the options. Call after
    /// anything that may change them: a configure, a plan approval's mode
    /// switch, a credential write, an `/agent:` rebind, or a turn, which may
    /// leave new skills, prompts, or MCP prompts behind.
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
        self.background.refresh(runtime);
        // Before the options, so a client knows whose they are.
        self.feed.update_agent(runtime.agent_id().as_str());
        self.feed
            .update_started_session_defaults(started_session_defaults(
                runtime.started_session_defaults(),
            ));
        let options = daemon_options(runtime, &self.config(), auths);
        self.feed.update_options(options.clone());
        self.feed.options()
    }
}

impl Sessions {
    /// Follows a credential write or a refreshed model list, which may
    /// change the models clients may pick: each `rho/model/list`
    /// subscriber builds its list again (see [`Self::model_changes`]), and
    /// every loaded session refreshes its options. Each refresh waits in
    /// its own task for the session's runtime, so a session in a turn or a
    /// configure refreshes once it lets the runtime go, even when its own
    /// refresh there read the auths before the write. The runtime's lock
    /// is fair, so refreshes for later writes queue after this one's.
    pub(in crate::app::daemon_host) async fn models_changed(&self) {
        self.model_lists.send_replace(());
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

    /// What [`Self::models_changed`] signals, for a `rho/model/list`
    /// subscriber to build its list again on.
    pub(in crate::app::daemon_host) fn model_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.model_lists.subscribe()
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

/// `defaults`, what the sessions a session starts open with, on the wire.
/// A mode this rho does not know is `Unknown`.
pub(super) fn started_session_defaults(
    defaults: &StartedSessionDefaults,
) -> wire::StartedSessionDefaults {
    let StartedSessionDefaults {
        permission_mode,
        model,
    } = defaults;
    wire::StartedSessionDefaults {
        permission_mode: permission_mode.as_ref().map(|mode| match mode {
            StartedMode::Known(mode) => (*mode).into(),
            StartedMode::Unknown(_) => wire::PermissionMode::Unknown,
        }),
        model: model
            .as_ref()
            .map(|StartedModel { provider, model }| wire::ModelRef {
                provider: provider.clone(),
                model: model.clone(),
            }),
    }
}
