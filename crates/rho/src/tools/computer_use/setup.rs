//! Installation is host-authorized separately from session desktop access.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::{anyhow, bail};
use futures_util::FutureExt;
use rho_sdk::CancellationToken;

use super::{policy::ManagedLocation, update, ComputerUseSession, State, Task};

mod installer;
pub(super) use installer::restrict_environment;

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
    /// The executable Rho launches now reports `version`.
    Updated {
        version: String,
    },
    Cancelled(InstallKind),
    Failed(InstallKind, String),
}

/// What a supervised installer run is for. Both share supervision and logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallKind {
    Install,
    /// Pinned to the release reported by the driver's own update check. Runs
    /// only while the launched driver still reports `from`, so a stale check
    /// can never downgrade a driver updated elsewhere.
    Update {
        from: String,
        to: String,
        location: ManagedLocation,
    },
}

pub(super) struct Installation {
    cancellation: Arc<CancellationToken>,
    task: Task<()>,
    kind: InstallKind,
    /// Set once the installer itself succeeded, before update verification.
    installed: Arc<AtomicBool>,
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

    /// The pending installer run, if any.
    pub(crate) fn pending_install(&self) -> Option<InstallKind> {
        match &*self.state() {
            State::Installing(installation) => Some(installation.kind.clone()),
            State::Off { .. }
            | State::Connecting { .. }
            | State::Connected { .. }
            | State::Closing { .. } => None,
        }
    }

    /// Called only after installation consent. Does not grant desktop access.
    pub(crate) fn start_installation(&self) -> anyhow::Result<PathBuf> {
        if self.driver_path().is_some() {
            bail!("a driver is already configured; run /computer setup again to connect without installing");
        }
        self.start_installer(InstallKind::Install)
    }

    /// Called only after update consent, with access off. Installs exactly the
    /// release the driver's own check reported, so the saved channel is kept.
    pub(crate) fn start_update(&self, from: &str, to: &str) -> anyhow::Result<PathBuf> {
        update::validate_version(from)?;
        update::validate_version(to)?;
        let location = self.ensure_managed_driver()?;
        self.start_installer(InstallKind::Update {
            from: from.to_owned(),
            to: to.to_owned(),
            location,
        })
    }

    /// The installer only replaces Cua's managed installation. Refuse before
    /// downloading anything when Rho would launch a different executable, such
    /// as an earlier PATH entry or a package-managed copy.
    pub(crate) fn ensure_managed_driver(&self) -> anyhow::Result<ManagedLocation> {
        let driver = self
            .driver_path()
            .ok_or_else(|| anyhow!("Cua Driver was not detected; /computer setup installs it"))?;
        let home = crate::paths::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| {
                anyhow!("an absolute home directory is required to update Cua Driver")
            })?;
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        super::policy::managed_driver_links(&home, local_app_data.as_deref())
            .into_iter()
            .find(|(_, link)| link.canonicalize().is_ok_and(|link| link == driver))
            .map(|(location, _)| location)
            .ok_or_else(|| {
                anyhow!(
                    "{} is not Cua's managed installation, so the installer would not replace it; update it the way it was installed",
                    driver.display()
                )
            })
    }

    /// The executable Rho launches must report `expected`; used before an
    /// update (stale check) and after it (installer did not take effect).
    pub(super) async fn require_driver_version(&self, expected: &str) -> anyhow::Result<()> {
        let driver = self
            .driver_path()
            .ok_or_else(|| anyhow!("Cua Driver is no longer detected"))?;
        let found = update::driver_version(&driver).await?;
        if found != expected {
            bail!("{} reports {found}, expected {expected}", driver.display());
        }
        Ok(())
    }

    fn start_installer(&self, kind: InstallKind) -> anyhow::Result<PathBuf> {
        let mut state = self.state();
        match &*state {
            State::Off { .. } => {}
            State::Installing(_) => {
                bail!("Cua Driver installation is already pending; /computer off cancels")
            }
            State::Connecting { .. } | State::Connected { .. } | State::Closing { .. } => {
                bail!("turn computer use off with /computer off before installing or updating Cua Driver")
            }
        }
        let home = crate::paths::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| {
                anyhow!("an absolute home directory is required to install Cua Driver")
            })?;
        let command = installer::command(&home, &kind)?;
        let (command, log) = installer::with_log(command)?;
        let cancellation = Arc::new(CancellationToken::new());
        let token = cancellation.clone();
        let log_path = log.clone();
        let session = self.clone();
        let task_kind = kind.clone();
        let installed = Arc::new(AtomicBool::new(false));
        let installer_done = installed.clone();
        let task = super::retained_task(async move {
            if let InstallKind::Update { from, .. } = &task_kind {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => bail!("Cua Driver update cancelled"),
                    result = session.require_driver_version(from) => result.map_err(|error| {
                        anyhow!("the driver changed since the update check ({error}); check again")
                    })?,
                }
            }
            installer::run(command, &token).await.map_err(|error| {
                anyhow!(
                    "{error}; installer log: {}. {INSTALLATION_RECOVERY}",
                    log_path.display()
                )
            })?;
            installer_done.store(true, Ordering::Release);
            let InstallKind::Update { to, .. } = &task_kind else {
                return Ok(());
            };
            // A cancelled verification reports failure, never success: the
            // files are replaced but the launched executable is unconfirmed.
            tokio::select! {
                biased;
                _ = token.cancelled() => bail!("verification cancelled"),
                result = session.require_driver_version(to) => result.map_err(|error| {
                    anyhow!("installer finished, but the update did not take effect: {error}; installer log: {}", log_path.display())
                }),
            }
        });
        // Any earlier check describes the executable being replaced.
        self.reset_update_check();
        *state = State::Installing(Installation {
            cancellation,
            task,
            kind,
            installed,
        });
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
        let installed = installation.installed.load(Ordering::Acquire);
        let kind = installation.kind.clone();
        *state = State::Off {
            error: None,
            revocation: None,
        };
        drop(state);
        Some(match (result, kind) {
            // Files are replaced, but the launched executable is unconfirmed.
            (_, kind @ InstallKind::Update { .. }) if cancelled && installed => {
                ComputerSetupUpdate::Failed(kind, "installer finished, but verification was cancelled; reopen /computer status to check the version".into())
            }
            (_, kind) if cancelled => ComputerSetupUpdate::Cancelled(kind),
            (Err(error), kind) => ComputerSetupUpdate::Failed(kind, error.to_string()),
            (Ok(()), InstallKind::Update { to, .. }) => {
                // The user just updated this driver; refresh what the dashboard shows.
                let _ = self.start_update_check();
                ComputerSetupUpdate::Updated { version: to }
            }
            (Ok(()), InstallKind::Install) if self.driver_path().is_some() => {
                ComputerSetupUpdate::Installed
            }
            (Ok(()), kind @ InstallKind::Install) => ComputerSetupUpdate::Failed(kind, "installer exited successfully but Cua Driver was not detected; check the installer log and run /computer setup again".into()),
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

/// A pending installation with no process behind it, for lifecycle tests.
#[cfg(test)]
pub(super) fn test_installation() -> Installation {
    Installation {
        cancellation: Arc::new(CancellationToken::new()),
        task: super::retained_task(std::future::pending()),
        kind: InstallKind::Install,
        installed: Arc::default(),
    }
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
