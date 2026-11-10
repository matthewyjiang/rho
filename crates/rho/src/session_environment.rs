//! The environment a session's children see (#1659): this process's own, or
//! the variables of the daemon client that last opened or prompted the
//! session. One [`SessionEnvironment`] reaches a session's tools, MCP
//! servers, hooks, OpenAPI sources, and CLI agents (Claude Code, Cursor,
//! Antigravity), and each takes the environment current when it starts a
//! child or reads a variable. A workflow run the session starts or resumes
//! instead gets its own handle of the environment current then, which no
//! later prompt replaces (see [`SessionEnvironment::snapshot`]).
//!
//! Project trust (`RHO_TRUST_PROJECT_*`) is read from the same environment,
//! once: a session keeps the trust of the environment it opened with, and so
//! do the workflow runs it starts (#1779). A host refuses a prompt whose
//! environment grants other trust (see [`SessionEnvironment::trust_mismatch`])
//! instead of running it under the session's. Only a [`SessionEnvironment`]
//! says what is trusted; an [`Ambient`] is variables.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    sync::{Arc, PoisonError, RwLock},
};

use rho_sdk::ProcessEnvironment;

use crate::workspace::ProjectTrusts;

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

    /// Gives `command` this whole environment, provider credentials
    /// included, for CLI agents such as Claude Code that sign in and run
    /// commands themselves: a local rho gives them its own environment, and
    /// a daemon session gives them its client's (#1659). This process's is
    /// inherited as is; a client's replaces it. Overrides set afterwards
    /// still apply.
    pub(crate) fn apply_full(&self, command: &mut tokio::process::Command) {
        match self {
            Self::ThisProcess => {}
            Self::Client(variables) => {
                command.env_clear().envs(variables.iter());
            }
        }
    }

    /// The `PATH` executables are looked up on.
    pub(crate) fn path(&self) -> Option<OsString> {
        self.read_os("PATH")
    }

    /// The environment tools' processes start with: this one without
    /// provider credentials, so commands cannot read API keys. This
    /// process's credential names are the ones set when this is called.
    fn process_environment(&self) -> ProcessEnvironment {
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
    /// process's that is not UTF-8, for passing straight to a child.
    pub(crate) fn read_os(&self, name: &str) -> Option<OsString> {
        match self {
            Self::ThisProcess => std::env::var_os(name),
            Self::Client(variables) => variables.get(name).map(OsString::from),
        }
    }

    /// A client environment of exactly `variables`, for tests that stand in
    /// for a daemon client.
    #[cfg(test)]
    pub(crate) fn client(variables: &[(&str, &str)]) -> Self {
        Self::Client(Arc::new(
            variables
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        ))
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
#[derive(Clone)]
pub(crate) struct SessionEnvironment {
    ambient: Arc<RwLock<Ambient>>,
    /// `ambient`'s process environment, the handle process tools snapshot.
    process: rho_tools::SharedProcessEnvironment,
    /// The trust the opening [`Ambient`] granted, which [`Self::replace`]
    /// keeps.
    project_trust: ProjectTrusts,
}

impl SessionEnvironment {
    /// A session's environment, starting as `ambient`, with the project
    /// trust `ambient` grants for as long as the session is open.
    pub(crate) fn new(ambient: Ambient) -> Self {
        Self {
            process: ambient.process_environment().into(),
            project_trust: ProjectTrusts::read(|name| ambient.read(name)),
            ambient: Arc::new(RwLock::new(ambient)),
        }
    }

    /// This environment as it is now, on a handle of its own that no later
    /// prompt replaces, with the session's project trust: what a workflow
    /// run the session starts or resumes runs in (#1659).
    pub(crate) fn snapshot(&self) -> Self {
        Self {
            project_trust: self.project_trust,
            ..Self::new(self.current())
        }
    }

    /// The environment of a session opened only to read what it saved: no
    /// client's variables, since nothing of the session starts, and the
    /// `project_trust` its host reads the project's definitions under.
    #[cfg_attr(
        not(unix),
        allow(dead_code, reason = "only the Unix daemon reads sessions")
    )]
    pub(crate) fn for_reading(project_trust: ProjectTrusts) -> Self {
        Self {
            project_trust,
            ..Self::new(Ambient::Client(Arc::default()))
        }
    }

    /// The session's project trust: what the environment it opened with
    /// granted, not what a later prompt's does, since the session loads its
    /// hooks, plugins, and agents when it opens (#1779).
    pub(crate) fn project_trust(&self) -> ProjectTrusts {
        self.project_trust
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

    /// How the project trust `ambient` grants differs from the session's,
    /// or `None` when they agree. A host asks before a client's prompt
    /// replaces the environment, and refuses the prompt on a difference: the
    /// session cannot change its trust while it is open, and would otherwise
    /// run the prompt with trust that client did not grant, or without
    /// trust it did (#1779).
    #[cfg_attr(not(unix), allow(dead_code, reason = "only the Unix daemon asks"))]
    pub(crate) fn trust_mismatch(&self, ambient: &Ambient) -> Option<TrustMismatch> {
        let theirs = ProjectTrusts::read(|name| ambient.read(name));
        (theirs != self.project_trust).then(|| TrustMismatch {
            session_only: self.project_trust.variables_beyond(theirs),
            other_only: theirs.variables_beyond(self.project_trust),
        })
    }

    /// Makes `ambient` the environment every later child sees, process
    /// tools' included. The session's project trust stays as it opened, so
    /// a host first refuses an `ambient` with a [`Self::trust_mismatch`].
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

/// How an environment's project trust differs from a session's (see
/// [`SessionEnvironment::trust_mismatch`]), as the `RHO_TRUST_PROJECT_*`
/// variables that grant what only one of them trusts. At least one list has
/// a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrustMismatch {
    /// What the session trusts and the other environment does not.
    pub(crate) session_only: Vec<&'static str>,
    /// What the other environment trusts and the session does not.
    pub(crate) other_only: Vec<&'static str>,
}

/// Shows the current [`Ambient`]'s redacted form, never a variable's value.
impl fmt::Debug for SessionEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionEnvironment")
            .field("ambient", &self.current())
            .finish_non_exhaustive()
    }
}

/// This process's own environment, which every session outside the daemon
/// keeps.
impl Default for SessionEnvironment {
    fn default() -> Self {
        Self::new(Ambient::ThisProcess)
    }
}

#[cfg(test)]
#[path = "session_environment_tests.rs"]
mod tests;
