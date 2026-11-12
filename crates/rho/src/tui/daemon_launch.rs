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
    /// `config.toml` as the launch read it ([`crate::app::read_config_file`]).
    pub(crate) config_file: std::sync::Arc<crate::config::Config>,
}

impl DaemonLaunch {
    /// `config.toml` as it is now, which a new session's options are
    /// built from, as the daemon will read it when the session opens, and
    /// the config it was settled to for the launch's agent. `None` when it
    /// does not load: the daemon refuses to open a new session on it and
    /// says why with its first prompt.
    pub(crate) fn config_now(
        &self,
    ) -> Option<(crate::config::Config, std::sync::Arc<crate::config::Config>)> {
        let read = crate::app::read_config_file().and_then(|file| {
            let settled = crate::app::config_for_new_session(&file, &self.definition)?;
            Ok((settled, std::sync::Arc::new(file)))
        });
        read.inspect_err(|error| {
            tracing::warn!(
                error = format!("{error:#}"),
                "could not read config.toml for a new session"
            );
        })
        .ok()
    }

    /// `config.toml` as it is now, which a setting chosen now is made
    /// against; the launch's read of it when it does not load.
    pub(crate) fn config_file_now(&self) -> std::sync::Arc<crate::config::Config> {
        crate::app::read_config_file()
            .map(std::sync::Arc::new)
            .unwrap_or_else(|_| std::sync::Arc::clone(&self.config_file))
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
