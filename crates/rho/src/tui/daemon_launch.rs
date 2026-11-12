//! What a launch asks of the daemon session it follows: how its sessions
//! open, which one it starts in, and the settings its flags name.

/// What a launch opens its sessions with: the one it starts in, and each
/// `/new` after it.
#[derive(Clone)]
pub(crate) struct DaemonLaunch {
    pub(crate) cwd: std::path::PathBuf,
    /// The agent a new session runs as; a resumed one runs as it was saved.
    pub(crate) agent: Option<String>,
    pub(crate) setup: rho_protocol::SessionSetup,
    /// The launch's settled config, which the session it starts in shows
    /// its options from until its first prompt opens it in the daemon.
    pub(crate) config: std::sync::Arc<crate::config::Config>,
    /// The agent the launch bound, whose model policy a session `/new`
    /// opens takes over `config.toml` as it is then.
    pub(crate) definition: std::sync::Arc<crate::agent::AgentDefinition>,
    /// The settings the launch's flags chose, whose values `config` holds:
    /// every new session takes them over `config.toml`.
    pub(crate) flags: crate::app::SettingOrigins,
    /// `config.toml` as the launch read it, under the agent's policy but
    /// not the flags: a choice made in a session the launch opened holds
    /// in the next one `/new` opens until `config.toml` changes it.
    pub(crate) config_file: std::sync::Arc<crate::config::Config>,
}

impl DaemonLaunch {
    /// The config a new session's options are built from now, as the
    /// daemon will read it when the session opens: `config.toml` as it is,
    /// so an edit since launch shows. One that does not load leaves the
    /// launch's read of it: the daemon refuses to open the session on it
    /// and says why with its first prompt.
    pub(crate) fn config_now(&self) -> crate::config::Config {
        crate::app::config_for_new_session(&self.definition).unwrap_or_else(|error| {
            tracing::warn!(
                error = format!("{error:#}"),
                "could not read config.toml for a new session"
            );
            (*self.config_file).clone()
        })
    }
}

/// Which session the TUI follows.
pub(crate) enum DaemonTarget {
    /// A new session, which the daemon opens with its first prompt.
    New,
    /// The saved session with this id, which the daemon reopens unless it
    /// has it loaded.
    Resume(rho_protocol::SessionId),
}

/// A change a launch's flags ask of the session it follows.
pub(crate) enum LaunchChange {
    Change(rho_protocol::SessionChange),
    /// `--auth` with neither `--provider` nor `--model`: run the session's
    /// own model on `auth` when `on`, the model the flag resolved to, is of
    /// the same provider, so resuming keeps the model it had; else `on`,
    /// as the auth is another provider's.
    Auth {
        auth: String,
        on: rho_protocol::ModelRef,
    },
}

impl LaunchChange {
    /// The change to send a session that runs with `options`.
    pub(super) fn of(self, options: &rho_protocol::SessionOptions) -> rho_protocol::SessionChange {
        match self {
            Self::Change(change) => change,
            Self::Auth { auth, on } => {
                let current = &options.model.current;
                rho_protocol::SessionChange::Model {
                    model: if current.provider == on.provider {
                        current.clone()
                    } else {
                        on
                    },
                    auth: Some(auth),
                    setup: true,
                }
            }
        }
    }
}

/// The settings a session starts to be followed with.
pub(crate) enum InitialSettings {
    /// As the daemon opened it.
    Kept,
    /// Changes the launch asked for; one the daemon refuses fails the open,
    /// unless the session has a turn under way, which they wait for.
    Required(Vec<LaunchChange>),
}
