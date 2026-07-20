use std::{env, fs};

/// Where clipboard operations should target for this process.
///
/// This is about which machine owns the user-facing clipboard. OAuth login uses a
/// related but different "can we open a browser?" check that also treats `HERDR_ENV`
/// as non-local; clipboard policy intentionally ignores that marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    /// Ordinary local desktop or server session.
    Local,
    /// SSH or Mosh. Native clipboard APIs would touch the remote host.
    Remote,
    /// WSL without SSH. The user-facing clipboard is usually the Windows host.
    Wsl,
}

impl SessionKind {
    pub fn detect() -> Self {
        Self::detect_from(env_var_present, is_wsl_host)
    }

    pub fn detect_from(has_env: impl Fn(&str) -> bool, is_wsl: impl Fn() -> bool) -> Self {
        const REMOTE_MARKERS: &[&str] = &["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "MOSH_IP"];
        if REMOTE_MARKERS.iter().copied().any(&has_env) {
            return Self::Remote;
        }
        if is_wsl() {
            return Self::Wsl;
        }
        Self::Local
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Wsl => "wsl",
        }
    }
}

pub(super) fn env_var_present(name: &str) -> bool {
    env::var_os(name).is_some()
}

pub(super) fn is_wsl_host() -> bool {
    env_var_present("WSL_DISTRO_NAME")
        || env_var_present("WSLENV")
        || fs::read_to_string("/proc/version")
            .map(|version| version.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
