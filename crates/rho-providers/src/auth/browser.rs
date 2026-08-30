//! Whether this process can show a desktop browser, and a best-effort open.
//!
//! The URL is always shown to the user. This module only decides whether to
//! exec a launcher. A wrong [`BrowserAvailability::Headless`] skips that exec;
//! a wrong [`BrowserAvailability::Graphical`] just fails and still leaves the
//! URL on screen.

/// Whether a graphical browser can reasonably appear for this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserAvailability {
    Graphical,
    Headless,
}

/// Injected facts for [`BrowserAvailability::resolve`].
///
/// Build with [`BrowserEnvironment::from_process`] at the process edge, or
/// construct directly in tests. Do not read the environment from `resolve`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrowserEnvironment {
    pub remote_shell: bool,
    pub display_server: bool,
    pub wsl_host: bool,
    pub nested_harness: bool,
}

impl BrowserEnvironment {
    /// Read the facts from the running process.
    pub fn from_process() -> Self {
        Self {
            remote_shell: remote_shell_from_process(),
            display_server: display_server_from_process(),
            wsl_host: wsl_host_from_process(),
            nested_harness: std::env::var_os("HERDR_ENV").is_some(),
        }
    }
}

impl BrowserAvailability {
    /// Conservative: prefer Headless when any signal says a window will not appear.
    ///
    /// A Linux display or a WSL host is enough to *attempt* a launch. `webbrowser`
    /// tries more than PATH launchers, so probing `xdg-open` here would skip a
    /// working Firefox on a minimal WM.
    pub fn resolve(environment: BrowserEnvironment) -> Self {
        if environment.remote_shell
            || environment.nested_harness
            || !(environment.display_server || environment.wsl_host)
        {
            Self::Headless
        } else {
            Self::Graphical
        }
    }

    /// Resolve availability from the running process.
    pub fn from_process() -> Self {
        Self::resolve(BrowserEnvironment::from_process())
    }
}

/// Result of a best-effort browser launch. Never an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserOpen {
    Launched,
    Skipped,
    Failed,
}

impl BrowserOpen {
    /// Status line presenters may show next to the authorize URL.
    pub fn note(self) -> &'static str {
        match self {
            Self::Launched => "opened in your browser",
            Self::Skipped => "open this URL on any machine",
            Self::Failed => "could not open a browser",
        }
    }
}

/// Open `url` when [`BrowserAvailability::Graphical`]; otherwise skip.
pub fn try_open(url: &str, availability: BrowserAvailability) -> BrowserOpen {
    match availability {
        BrowserAvailability::Headless => BrowserOpen::Skipped,
        BrowserAvailability::Graphical => match webbrowser::open(url) {
            Ok(()) => BrowserOpen::Launched,
            Err(_) => BrowserOpen::Failed,
        },
    }
}

fn remote_shell_from_process() -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "MOSH_IP"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

fn display_server_from_process() -> bool {
    #[cfg(any(windows, target_os = "macos"))]
    {
        true
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
}

fn wsl_host_from_process() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSLENV").is_some()
        || std::fs::read_to_string("/proc/version")
            .map(|version| version.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
