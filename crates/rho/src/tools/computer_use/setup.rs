//! Installation is host-authorized separately from session desktop access.

use std::{path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail};
use futures_util::FutureExt;
use rho_sdk::CancellationToken;

use super::{ComputerUseSession, State, Task};

mod installer;

/// Platform-specific installation disclosure shared by CLI guidance and consent.
pub(crate) struct ComputerSetupPlatform {
    pub source: &'static str,
    pub locations: &'static str,
    pub notes: &'static str,
}

pub(crate) fn setup_platform() -> ComputerSetupPlatform {
    if cfg!(windows) {
        ComputerSetupPlatform {
            source: "https://cua.ai/driver/install.ps1 with PowerShell",
            locations: "%USERPROFILE%\\.cua-driver, with the executable in %USERPROFILE%\\.cua-driver\\bin",
            notes: "Rho requests -NoAutoStart, but the installer may re-register an existing cua-driver-serve task and prompt for UAC elevation. Elevated work may run outside Rho's supervision and continue after cancellation; task changes are not rolled back.",
        }
    } else if cfg!(target_os = "macos") {
        ComputerSetupPlatform {
            source: "https://cua.ai/driver/install.sh with Bash",
            locations: "~/.cua-driver and /Applications/CuaDriver.app, with a link in ~/.local/bin",
            notes: "After the daemon is running, use cua-driver permissions status and grant Accessibility and Screen Recording in System Settings as needed.",
        }
    } else {
        ComputerSetupPlatform {
            source: "https://cua.ai/driver/install.sh with Bash",
            locations: "~/.cua-driver, with a link in ~/.local/bin",
            notes: "Launch Rho from the desktop session you intend to control.",
        }
    }
}

pub(crate) const INSTALLATION_RECOVERY: &str = "Partial files may remain and the saved telemetry preference may be unchanged. If /computer setup detects the driver, it skips installation and does not retry the saved opt-out; run cua-driver telemetry disable manually to save it. Rho still forces telemetry off for managed connections.";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ComputerSetupUpdate {
    Installed,
    Cancelled,
    Failed(String),
}

pub(super) struct Installation {
    cancellation: Arc<CancellationToken>,
    task: Task<()>,
}

impl Installation {
    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl ComputerUseSession {
    pub(crate) fn installation_pending(&self) -> bool {
        matches!(&*self.state(), State::Installing(_))
    }

    /// Called only after installation consent. Does not grant desktop access.
    pub(crate) fn start_installation(&self) -> anyhow::Result<PathBuf> {
        let mut state = self.state();
        match &*state {
            State::Off { .. } => {}
            State::Installing(_) => {
                bail!("Cua Driver installation is already pending; /computer off cancels")
            }
            State::Connecting { .. } | State::Connected { .. } | State::Closing { .. } => {
                bail!("turn computer use off before installing Cua Driver")
            }
        }
        if self.driver_path().is_some() {
            bail!("a driver is already configured; run /computer setup again to connect without installing");
        }
        let home = crate::paths::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| {
                anyhow!("an absolute home directory is required to install Cua Driver")
            })?;
        let command = installer::command(&home)?;
        let (command, log) = installer::with_log(command)?;
        let cancellation = Arc::new(CancellationToken::new());
        let token = cancellation.clone();
        let log_path = log.clone();
        let task = super::retained_task(async move {
            installer::run(command, &token).await.map_err(|error| {
                anyhow!(
                    "{error}; installer log: {}. {INSTALLATION_RECOVERY}",
                    log_path.display()
                )
            })
        });
        *state = State::Installing(Installation { cancellation, task });
        Ok(log)
    }

    /// Keep the lifecycle occupied until the cancelled process tree is reaped.
    pub(super) async fn finish_installation(&self) {
        let (cancellation, task) = match &*self.state() {
            State::Installing(installation) => {
                (installation.cancellation.clone(), installation.task.clone())
            }
            State::Off { .. }
            | State::Connecting { .. }
            | State::Connected { .. }
            | State::Closing { .. } => return,
        };
        let _ = task.await;
        let mut state = self.state();
        if matches!(&*state, State::Installing(current) if Arc::ptr_eq(&current.cancellation, &cancellation))
        {
            *state = State::Off {
                error: None,
                revocation: None,
            };
        }
    }

    pub(crate) fn take_installation_result(&self) -> Option<ComputerSetupUpdate> {
        let mut state = self.state();
        let installation = match &*state {
            State::Installing(installation) => installation,
            State::Off { .. }
            | State::Connecting { .. }
            | State::Connected { .. }
            | State::Closing { .. } => return None,
        };
        let result = installation.task.clone().now_or_never()?;
        let cancelled = installation.cancellation.is_cancelled();
        *state = State::Off {
            error: None,
            revocation: None,
        };
        Some(if cancelled {
            ComputerSetupUpdate::Cancelled
        } else {
            match result {
                Ok(()) if self.driver_path().is_some() => ComputerSetupUpdate::Installed,
                Ok(()) => ComputerSetupUpdate::Failed("installer exited successfully but Cua Driver was not detected; check the installer log and run /computer setup again".into()),
                Err(error) => ComputerSetupUpdate::Failed(error.to_string()),
            }
        })
    }

    pub(crate) fn setup_guidance() -> String {
        let ComputerSetupPlatform {
            source,
            locations,
            notes,
        } = setup_platform();
        format!("Run /computer setup inside Rho to detect Cua Driver, install it if missing after separate installation consent, then review desktop access and verify the driver connection. Access consent is saved on this machine for future interactive sessions until /computer off. No MCP config is written.\n\nThe installer runs {source} and writes to {locations}. {notes}\n\nRho forces Cua telemetry off before installation and every managed driver launch, overriding caller telemetry settings. After installation it runs cua-driver telemetry disable to save the opt-out. Existing drivers are not run without current or saved desktop consent; their saved telemetry preferences are unchanged. Rho requests no PATH or shell profile changes. Installation does not grant Rho desktop access.\n\nIf installation fails or is cancelled: {INSTALLATION_RECOVERY}\n\nDriver handshake is not an OS permission check. Run cua-driver doctor for installation diagnostics.\n\nOfficial setup: https://cua.ai/docs/how-to-guides/driver/install\n/computer off cancels supervised installation processes, revokes access, and disables future connections; it cannot undo installation files or completed desktop actions.")
    }
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
