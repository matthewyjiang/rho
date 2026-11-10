//! What `rho doctor` observes about the daemon (#1674), from outside and
//! without ever starting one: the handshake and status of the one running,
//! the socket's permissions, the documented service's files, and, where the
//! OS shows it, the daemon's environment. `crate::doctor::daemon` turns each
//! observation into a row.
//!
//! This is a boundary that reads the process environment and the user's
//! files; the functions it calls take what was read.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use rho_protocol::{methods::Status, ClientError, StatusParams};

use crate::{
    daemon::{self, ConnectError, DaemonPathError, DaemonPaths, SocketFound, VariableKind},
    doctor::{
        daemon::{
            environment_check, process_check, service_check, socket_check, DaemonEnvironment,
            DaemonProcess, DaemonService, DaemonSocket,
        },
        daemon_store_check, DaemonStore, DoctorCheck,
    },
};

/// Every daemon row, asking the running daemon for at most `deadline`.
pub(super) async fn checks(deadline: Duration) -> Vec<DoctorCheck> {
    let service = match crate::paths::home_dir() {
        Some(home) => find_service(
            &home,
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        ),
        None => DaemonService::Unknown("could not determine the home directory".into()),
    };
    let (process, store, socket, environment) = match super::daemon_cli::paths() {
        Ok(paths) => {
            let (process, store) = ask(&paths, deadline).await;
            let environment = match daemon_pid(&process) {
                None => DaemonEnvironment::NotRunning,
                Some(pid) => match read_environment(pid) {
                    Ok(daemon) => compare_environments(&daemon, |name| std::env::var_os(name)),
                    Err(reason) => DaemonEnvironment::Unknown(reason),
                },
            };
            (process, store, inspect_socket(&paths), environment)
        }
        Err(error) => {
            let reason = format!("{error:#}");
            (
                DaemonProcess::Unanswered(reason.clone()),
                DaemonStore::Unanswered(reason.clone()),
                DaemonSocket::Unknown(reason.clone()),
                DaemonEnvironment::Unknown(reason),
            )
        }
    };
    vec![
        process_check(&process, env!("CARGO_PKG_VERSION")),
        socket_check(&socket),
        service_check(&service),
        environment_check(&environment),
        daemon_store_check(&store),
    ]
}

/// Completes the handshake with the running daemon and asks its status,
/// giving up after `deadline`. The status carries the credential store the
/// daemon keeps provider credentials in and whether it can reach it.
async fn ask(paths: &DaemonPaths, deadline: Duration) -> (DaemonProcess, DaemonStore) {
    let unanswered = |reason: String| {
        (
            DaemonProcess::Unanswered(reason.clone()),
            DaemonStore::Unanswered(reason),
        )
    };
    let ask = async {
        let connection = match daemon::connect(paths, daemon::ConnectAs::Command.hello()).await {
            Ok(connection) => connection,
            Err(ConnectError::NotRunning) => {
                return (DaemonProcess::NotRunning, DaemonStore::NotRunning)
            }
            Err(ConnectError::Handshake(ClientError::VersionMismatch(mismatch))) => {
                let recorded =
                    daemon::running_record(paths).map(|record| (record.pid, record.version));
                return (
                    DaemonProcess::Incompatible { mismatch, recorded },
                    DaemonStore::Unanswered(mismatch.to_string()),
                );
            }
            Err(error) => return unanswered(error.to_string()),
        };
        let process = DaemonProcess::Running {
            pid: connection.hello.daemon.pid,
            version: connection.hello.daemon.version.clone(),
            protocol: connection.hello.protocol,
        };
        let store = match connection.client.request::<Status>(&StatusParams {}).await {
            Ok(status) => DaemonStore::Reported(status.credential_store),
            Err(error) => DaemonStore::Unanswered(error.to_string()),
        };
        (process, store)
    };
    match tokio::time::timeout(deadline, ask).await {
        Ok(answer) => answer,
        Err(_) => unanswered(format!(
            "it did not report its status within PROBE_DEADLINE ({deadline:?})"
        )),
    }
}

/// The pid of the daemon running now, when the handshake or its info file
/// named one.
fn daemon_pid(process: &DaemonProcess) -> Option<u32> {
    match process {
        DaemonProcess::Running { pid, .. } => Some(*pid),
        DaemonProcess::Incompatible { recorded, .. } => recorded.as_ref().map(|(pid, _)| *pid),
        DaemonProcess::NotRunning | DaemonProcess::Unanswered(_) => None,
    }
}

fn inspect_socket(paths: &DaemonPaths) -> DaemonSocket {
    let socket = paths.socket.display().to_string();
    match paths.inspect_socket() {
        Ok(SocketFound::Missing) => DaemonSocket::Missing { socket },
        Ok(SocketFound::Private) => DaemonSocket::Private { socket },
        Err(error @ DaemonPathError::Io { .. }) => DaemonSocket::Unknown(error.to_string()),
        Err(
            error @ (DaemonPathError::SocketPathTooLong { .. }
            | DaemonPathError::NotADirectory { .. }
            | DaemonPathError::WrongOwner { .. }
            | DaemonPathError::InsecureMode { .. }
            | DaemonPathError::NotASocket { .. }
            | DaemonPathError::SocketWrongOwner { .. }
            | DaemonPathError::InsecureSocketMode { .. }),
        ) => DaemonSocket::Exposed(error.to_string()),
    }
}

/// Looks for the service `docs/daemon.md` has the user save, under the name
/// it gives, in the user's own unit directory. This reads files only, so it
/// also answers where no user service manager can be asked, as in an SSH
/// login without a session bus.
#[cfg(target_os = "linux")]
fn find_service(home: &Path, xdg_config_home: Option<PathBuf>) -> DaemonService {
    // XDG: a relative value is invalid and must be ignored.
    let units = xdg_config_home
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("systemd/user");
    use crate::doctor::daemon::SYSTEMD_UNIT;

    if !units.join(SYSTEMD_UNIT).exists() {
        return DaemonService::NotInstalled;
    }
    // Enabled when a target's `.wants` or `.requires` directory links to
    // the unit, which is what `systemctl --user enable` creates.
    let enabled = std::fs::read_dir(&units).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let dir = entry.path();
            let pulls_in_units = dir
                .extension()
                .is_some_and(|extension| extension == "wants" || extension == "requires");
            // The link's target may be anywhere, so only its own existence
            // counts.
            pulls_in_units && dir.join(SYSTEMD_UNIT).symlink_metadata().is_ok()
        })
    });
    DaemonService::Systemd { enabled }
}

/// Looks for the LaunchAgent `docs/daemon.md` has the user save, under the
/// label it gives.
#[cfg(target_os = "macos")]
fn find_service(home: &Path, _xdg_config_home: Option<PathBuf>) -> DaemonService {
    let plist = home
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", crate::doctor::daemon::LAUNCHD_LABEL));
    if plist.exists() {
        DaemonService::Launchd
    } else {
        DaemonService::NotInstalled
    }
}

/// The docs cover no service setup for this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn find_service(_home: &Path, _xdg_config_home: Option<PathBuf>) -> DaemonService {
    DaemonService::NotInstalled
}

/// Sorts the variables that differ between `daemon`'s environment and the
/// `client` lookup by whether they change what the daemon does.
fn compare_environments(
    daemon: &[(OsString, OsString)],
    client: impl Fn(&str) -> Option<OsString>,
) -> DaemonEnvironment {
    let (mut acts, mut machine) = (Vec::new(), Vec::new());
    for (name, kind) in daemon::differing_variables(daemon, client) {
        match kind {
            VariableKind::Network | VariableKind::Setting => acts.push(name.to_owned()),
            VariableKind::Machine => machine.push(name.to_owned()),
        }
    }
    DaemonEnvironment::Compared { acts, machine }
}

/// The environment process `pid` was started with, from `/proc`, which
/// shows it to the process's own user. The daemon reports none over the
/// protocol, and it never changes its own.
#[cfg(target_os = "linux")]
fn read_environment(pid: u32) -> Result<Vec<(OsString, OsString)>, String> {
    let path = format!("/proc/{pid}/environ");
    std::fs::read(&path)
        .map(|bytes| parse_environ(&bytes))
        .map_err(|error| format!("could not read {path}: {error}"))
}

#[cfg(not(target_os = "linux"))]
fn read_environment(_pid: u32) -> Result<Vec<(OsString, OsString)>, String> {
    Err("this platform does not show a running process's environment, and the daemon does not report its own".into())
}

/// Parses `/proc/<pid>/environ`: `NAME=value` entries, each ended by a NUL.
#[cfg(target_os = "linux")]
fn parse_environ(bytes: &[u8]) -> Vec<(OsString, OsString)> {
    use std::os::unix::ffi::OsStrExt;

    bytes
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let equals = entry.iter().position(|byte| *byte == b'=')?;
            let (name, value) = (&entry[..equals], &entry[equals + 1..]);
            Some((
                std::ffi::OsStr::from_bytes(name).to_owned(),
                std::ffi::OsStr::from_bytes(value).to_owned(),
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "doctor_daemon_tests.rs"]
mod tests;
