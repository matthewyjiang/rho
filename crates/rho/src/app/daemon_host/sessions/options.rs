//! A daemon session's options on the wire: its settings and their choices,
//! and the commands its prompts run, which the ACP adapter (#1670) and the
//! TUI client (#1667) render. [`crate::app::session_options`] builds them
//! with the daemon's commands; this keeps each session's feed current.

use std::sync::Arc;

use rho_protocol as wire;

use super::{turn::DAEMON_COMMANDS, LoadedSession, Sessions};
use crate::{
    app::{session_options, session_runtime::SessionRuntime},
    config::Config,
};

impl LoadedSession {
    /// Rebuilds the session's options from `runtime`, tells attached
    /// clients when they changed, and returns them. Call after anything
    /// that may change them: a configure, a plan approval's mode switch, a
    /// credential write, or a turn, which may leave new skills, prompts, or
    /// MCP prompts behind.
    pub(super) fn refresh_options(&self, runtime: &SessionRuntime) -> wire::SessionOptions {
        let options = daemon_options(runtime, &self.config);
        self.feed.update_options(options.clone());
        options
    }
}

impl Sessions {
    /// Refreshes the options of every loaded session whose runtime is free,
    /// after a credential write changed the models they may switch to. A
    /// session in a turn or a configure refreshes at its end instead.
    pub(in crate::app::daemon_host) fn refresh_idle_options(&self) {
        let sessions: Vec<Arc<LoadedSession>> = self.loaded().values().cloned().collect();
        for session in sessions {
            if let Ok(runtime) = session.runtime.try_lock() {
                // `None` once the daemon's stop took it.
                if let Some(runtime) = runtime.as_ref() {
                    session.refresh_options(runtime);
                }
            }
        }
    }
}

/// `runtime`'s options now, with the choices `process_config` gives it.
pub(super) fn daemon_options(
    runtime: &SessionRuntime,
    process_config: &Config,
) -> wire::SessionOptions {
    session_options::of(runtime, process_config, DAEMON_COMMANDS)
}
