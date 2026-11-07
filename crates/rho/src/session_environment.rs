//! The environment a session's children see (#1659): this process's own, or
//! the variables of the daemon client that last opened or prompted the
//! session. One [`SessionEnvironment`] reaches a session's tools, MCP
//! servers, hooks, and OpenAPI sources, and each takes the environment
//! current when it starts a child or reads a variable.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    sync::{Arc, PoisonError, RwLock},
};

use rho_sdk::ProcessEnvironment;

/// The environment a session's children see (#1659): this process's own,
/// or a daemon client's variables.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Ambient {
    /// This process's environment, read when a child starts.
    ThisProcess,
    /// A daemon client's variables, all of them, provider credentials
    /// included.
    #[cfg_attr(
        not(unix),
        allow(dead_code, reason = "only the Unix daemon has clients")
    )]
    Client(Arc<BTreeMap<String, String>>),
}

impl Ambient {
    /// Variable `name`, for a list of names the user configured, such as a
    /// hook's `env` or an MCP server's `env_from_env`. Provider credentials
    /// are included: the user named them deliberately.
    pub(crate) fn read(&self, name: &str) -> Option<String> {
        match self {
            Self::ThisProcess => std::env::var(name).ok(),
            Self::Client(variables) => variables.get(name).cloned(),
        }
    }

    /// Clears `command`'s environment and installs the sanitized base set of
    /// [`crate::child_env`], read from this environment.
    pub(crate) fn apply_base(&self, command: &mut tokio::process::Command) {
        crate::child_env::apply_base(command, |name| self.read_os(name));
    }

    /// The `PATH` executables are looked up on.
    pub(crate) fn path(&self) -> Option<OsString> {
        self.read_os("PATH")
    }

    /// The environment tools' processes start with: this one without
    /// provider credentials, so commands cannot read API keys. This
    /// process's credential names are the ones set when this is called.
    pub(crate) fn process_environment(&self) -> ProcessEnvironment {
        match self {
            Self::ThisProcess => {
                ProcessEnvironment::inherit_except(rho_providers::credential_env_vars())
            }
            Self::Client(variables) => ProcessEnvironment::Explicit {
                variables: variables
                    .iter()
                    .filter(|(name, _)| !rho_providers::is_credential_env_var(name))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            },
        }
    }

    /// Variable `name` as [`Self::read`] reads it, keeping a value of this
    /// process's that is not UTF-8.
    fn read_os(&self, name: &str) -> Option<OsString> {
        match self {
            Self::ThisProcess => std::env::var_os(name),
            Self::Client(variables) => variables.get(name).map(OsString::from),
        }
    }
}

/// Shows how many variables a client sent, never their values.
impl fmt::Debug for Ambient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThisProcess => formatter.write_str("ThisProcess"),
            Self::Client(variables) => formatter
                .debug_struct("Client")
                .field("len", &variables.len())
                .finish_non_exhaustive(),
        }
    }
}

/// The session's [`Ambient`], which its host may replace between turns;
/// clones share it. A daemon session's is replaced by each prompt with the
/// prompting client's (#1659).
#[derive(Clone, Debug)]
pub(crate) struct SessionEnvironment {
    ambient: Arc<RwLock<Ambient>>,
    /// `ambient`'s process environment, the handle process tools snapshot.
    process: rho_tools::SharedProcessEnvironment,
}

impl SessionEnvironment {
    pub(crate) fn new(ambient: Ambient) -> Self {
        Self {
            process: ambient.process_environment().into(),
            ambient: Arc::new(RwLock::new(ambient)),
        }
    }

    /// The environment a child started now sees.
    pub(crate) fn current(&self) -> Ambient {
        self.ambient
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The handle the session's process tools take their environment from.
    pub(crate) fn process(&self) -> rho_tools::SharedProcessEnvironment {
        self.process.clone()
    }

    /// Makes `ambient` the environment every later child sees, process
    /// tools' included.
    #[cfg_attr(
        not(unix),
        allow(dead_code, reason = "only the Unix daemon replaces it")
    )]
    pub(crate) fn replace(&self, ambient: Ambient) {
        let mut current = self.ambient.write().unwrap_or_else(PoisonError::into_inner);
        self.process.replace(ambient.process_environment());
        *current = ambient;
    }
}

#[cfg(test)]
#[path = "session_environment_tests.rs"]
mod tests;
