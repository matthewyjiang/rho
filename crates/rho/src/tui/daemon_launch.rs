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
}

/// Which session the TUI follows.
pub(crate) enum DaemonTarget {
    /// A new session the daemon opens.
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

#[cfg(unix)]
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
    /// The settings of the session it replaces; one the daemon refuses
    /// keeps its default.
    Carried(Box<rho_protocol::SessionOptions>),
}
