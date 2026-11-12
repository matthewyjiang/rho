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
    daemon::{
        self, ConnectError, DaemonPaths, LockHolder, ServiceManager, SocketFound, VariableKind,
    },
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
            // A daemon that did not answer still wrote its record.
            let pid = daemon_pid(&process).or_else(|| {
                daemon::lock_holder(&paths)
                    .record()
                    .map(|record| record.pid)
            });
            let environment = match (&process, pid) {
                (DaemonProcess::NotRunning, _) => DaemonEnvironment::NotRunning,
                (_, Some(pid)) => match read_environment(pid) {
                    Ok(daemon) => compare_environments(&daemon, |name| std::env::var_os(name)),
                    Err(reason) => DaemonEnvironment::Unknown(reason),
                },
                (_, None) => {
                    DaemonEnvironment::Unknown("the daemon did not say which process it is".into())
                }
            };
            // The socket that matters is the one the daemon listens on.
            let socket = match &process {
                DaemonProcess::OtherSocket { socket, .. } => {
                    let socket = PathBuf::from(socket);
                    inspect_socket(&DaemonPaths {
                        runtime_dir: socket.parent().unwrap_or(&socket).to_path_buf(),
                        socket,
                        ..paths.clone()
                    })
                }
                DaemonProcess::NotRunning
                | DaemonProcess::Running { .. }
                | DaemonProcess::Incompatible { .. }
                | DaemonProcess::Unanswered(_) => inspect_socket(&paths),
            };
            (process, store, socket, environment)
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
            Err(ConnectError::NotRunning) => return not_answering(paths),
            Err(ConnectError::Handshake(ClientError::VersionMismatch(mismatch))) => {
                let recorded = daemon::lock_holder(paths)
                    .record()
                    .map(|record| (record.pid, record.version));
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

/// What to report when nothing listens on this process's socket: a daemon
/// that holds the home's lock all the same is one listening on a socket of
/// another runtime directory, or one still starting or stopping.
fn not_answering(paths: &DaemonPaths) -> (DaemonProcess, DaemonStore) {
    let record = match daemon::lock_holder(paths) {
        LockHolder::Nobody => return (DaemonProcess::NotRunning, DaemonStore::NotRunning),
        LockHolder::Unrecorded => {
            let reason =
                "it is starting or stopping; run `rho doctor` again in a moment".to_owned();
            return (
                DaemonProcess::Unanswered(reason.clone()),
                DaemonStore::Unanswered(reason),
            );
        }
        LockHolder::Daemon(record) => record,
    };
    let (process, reason) = if record.socket == paths.socket {
        let reason = format!(
            "pid {} holds the daemon lock but does not listen yet or any more",
            record.pid
        );
        (DaemonProcess::Unanswered(reason.clone()), reason)
    } else {
        (
            DaemonProcess::OtherSocket {
                pid: record.pid,
                socket: record.socket.display().to_string(),
                expected: paths.socket.display().to_string(),
            },
            "it listens on another socket".to_owned(),
        )
    };
    (process, DaemonStore::Unanswered(reason))
}

/// The pid of the daemon running now, when the handshake or its info file
/// named one.
fn daemon_pid(process: &DaemonProcess) -> Option<u32> {
    match process {
        DaemonProcess::Running { pid, .. } | DaemonProcess::OtherSocket { pid, .. } => Some(*pid),
        DaemonProcess::Incompatible { recorded, .. } => recorded.as_ref().map(|(pid, _)| *pid),
        DaemonProcess::NotRunning | DaemonProcess::Unanswered(_) => None,
    }
}

fn inspect_socket(paths: &DaemonPaths) -> DaemonSocket {
    let socket = paths.socket.display().to_string();
    match paths.inspect_socket() {
        Ok(SocketFound::Missing) => DaemonSocket::Missing { socket },
        Ok(SocketFound::Private) => DaemonSocket::Private { socket },
        Err(error) if error.is_unreadable() => DaemonSocket::Unknown(error.to_string()),
        Err(error) => DaemonSocket::Exposed(error.to_string()),
    }
}

/// The service `docs/daemon.md` has the user save, the systemd user unit
/// before the LaunchAgent, as its files show it (see
/// [`daemon::find_services`]).
fn find_service(home: &Path, xdg_config_home: Option<PathBuf>) -> DaemonService {
    match daemon::find_services(home, xdg_config_home).first() {
        None => DaemonService::NotInstalled,
        Some(service) => match service.manager {
            ServiceManager::Systemd => DaemonService::Systemd {
                enabled: !service.enabled_by.is_empty(),
            },
            ServiceManager::Launchd => DaemonService::Launchd,
        },
    }
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
