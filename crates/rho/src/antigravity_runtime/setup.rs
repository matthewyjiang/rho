//! Antigravity install and sign-in state, as `/doctor` and `/info` report it.
//!
//! Reads only the filesystem and environment: starting `agy_acp_server` to
//! ask for its version means loading a 926 MB binary (initialize took 1.4 to
//! 2.1 s warm in the 1.3.0 spike), too heavy for a status row.

use std::path::{Path, PathBuf};

use super::{
    executable::{self, HarnessLocation, ANTIGRAVITY_PROGRAM},
    home::{AntigravityAuthStatus, AntigravityHome},
    ANTIGRAVITY_LABEL_NAME,
};

/// The server binary as the next run would find it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServerInstall {
    /// Not on `PATH`; runs fail before spawning.
    Missing,
    Found {
        path: PathBuf,
        harness: HarnessLocation,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AntigravitySetup {
    pub(crate) server: ServerInstall,
    pub(crate) auth: AntigravityAuthStatus,
}

impl AntigravitySetup {
    /// Snapshot from `PATH`, `ANTIGRAVITY_HARNESS_PATH`, and the Gemini home.
    pub(crate) fn from_env(home: &Path) -> Self {
        let server = match executable::resolve() {
            Ok(found) => ServerInstall::Found {
                harness: executable::harness_location(
                    found.path(),
                    executable::harness_override_from_env(),
                ),
                path: found.path().to_path_buf(),
            },
            Err(_) => ServerInstall::Missing,
        };
        Self {
            server,
            auth: AntigravityHome::from_env(home).status(),
        }
    }

    /// One `/info` line. An install problem outranks sign-in state because
    /// runs fail on it first.
    pub(crate) fn description(&self) -> String {
        let state = match &self.server {
            ServerInstall::Missing => format!("{ANTIGRAVITY_PROGRAM} not found on PATH"),
            ServerInstall::Found {
                harness: HarnessLocation::Missing { expected },
                ..
            } => format!("{} is missing", crate::paths::display(expected)),
            ServerInstall::Found {
                harness: HarnessLocation::Override | HarnessLocation::Beside(_),
                ..
            } => match &self.auth {
                AntigravityAuthStatus::Configured { method } => format!("signed in ({method})"),
                AntigravityAuthStatus::SignedOut { .. }
                | AntigravityAuthStatus::MissingToken { .. } => {
                    "not signed in - run /login antigravity".into()
                }
                AntigravityAuthStatus::Unreadable { settings, detail } => format!(
                    "could not read {}: {detail}",
                    crate::paths::display(settings)
                ),
            },
        };
        format!("{ANTIGRAVITY_LABEL_NAME}: {state}")
    }
}
