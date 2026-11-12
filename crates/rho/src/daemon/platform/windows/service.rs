//! Windows has no manager for the services `docs/daemon.md` documents,
//! which are systemd's and launchd's.

use std::io;

use crate::daemon::service::ServiceManager;

/// Fails: neither manager runs here, so a file found under one's name was
/// brought from another machine and nothing runs it.
pub(in crate::daemon) fn stop_service(manager: ServiceManager) -> io::Result<()> {
    let name = match manager {
        ServiceManager::Systemd => "systemd",
        ServiceManager::Launchd => "launchd",
    };
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{name} does not run on Windows"),
    ))
}
