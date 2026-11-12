//! The service files `docs/daemon.md` has a user save for
//! `rho mcp serve --http`, the one process that needs a service: the daemon
//! has no unit of its own, and no install method writes one. `rho doctor`
//! reports them and `rho uninstall` removes them (#1666).
//!
//! Finding them reads files only, so it also answers where no user service
//! manager can be asked, as in an SSH login without a session bus. Each is
//! looked for on every platform, as the other's file is simply not there.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::platform;

/// The systemd user unit `docs/daemon.md` has you save.
pub(crate) const SYSTEMD_UNIT: &str = "rho-mcp.service";
/// The launchd label of the same service on macOS.
pub(crate) const LAUNCHD_LABEL: &str = "dev.rho.mcp";

/// What runs a saved service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceManager {
    Systemd,
    Launchd,
}

/// One saved service, under the name the docs give it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServiceFiles {
    pub(crate) manager: ServiceManager,
    /// The unit or the agent's property list.
    pub(crate) definition: PathBuf,
    /// The links that make a target start the unit, which
    /// `systemctl --user enable` creates. launchd has none.
    pub(crate) enabled_by: Vec<PathBuf>,
}

/// The services saved in `home`, the systemd unit first. `xdg_config_home`
/// is `XDG_CONFIG_HOME`; a relative value is invalid there and is ignored.
pub(crate) fn find_services(home: &Path, xdg_config_home: Option<PathBuf>) -> Vec<ServiceFiles> {
    let mut found = Vec::new();
    let units = xdg_config_home
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("systemd/user");
    let unit = units.join(SYSTEMD_UNIT);
    if unit.exists() {
        // A target's `.wants` or `.requires` directory links to the unit.
        // The link's target may be anywhere, so only its own existence
        // counts.
        let mut enabled_by: Vec<PathBuf> = fs::read_dir(&units)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| {
                dir.extension()
                    .is_some_and(|extension| extension == "wants" || extension == "requires")
            })
            .map(|dir| dir.join(SYSTEMD_UNIT))
            .filter(|link| link.symlink_metadata().is_ok())
            .collect();
        enabled_by.sort();
        found.push(ServiceFiles {
            manager: ServiceManager::Systemd,
            definition: unit,
            enabled_by,
        });
    }
    let agent = home
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"));
    if agent.exists() {
        found.push(ServiceFiles {
            manager: ServiceManager::Launchd,
            definition: agent,
            enabled_by: Vec::new(),
        });
    }
    found
}

impl ServiceFiles {
    /// Every file of the service, the links before what they point to.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.enabled_by
            .iter()
            .chain([&self.definition])
            .map(PathBuf::as_path)
    }

    /// Asks the service's manager to stop it and no longer start it, so
    /// nothing runs a `rho` that is about to be removed. Fails when the
    /// manager cannot be asked or refuses; the files can be deleted all the
    /// same, and the error says what is left to do by hand.
    pub(crate) fn stop(&self) -> io::Result<()> {
        platform::stop_service(self.manager)
    }

    /// Deletes the service's files. One already gone is not an error: the
    /// manager removes the links itself as it disables the unit.
    pub(crate) fn delete(&self) -> io::Result<()> {
        for path in self.paths() {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
