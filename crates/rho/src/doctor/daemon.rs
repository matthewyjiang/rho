//! The doctor rows about the rho daemon itself (#1674): whether one runs and
//! matches this `rho`, whether its socket is private, whether the documented
//! always-on service is installed, and how the daemon's environment differs
//! from this shell's. `daemon_observe` observes each from outside, never
//! starting a daemon; the credential store row is in `daemon_store`.

use rho_protocol::{ProtocolVersion, RestartCost, VersionFix, VersionMismatch};

use super::report::{DoctorCheck, DoctorCheckId, DoctorStatus};

use crate::daemon::{LAUNCHD_LABEL, SYSTEMD_UNIT};

const SERVICE_DOCS: &str = "https://matthewyjiang.github.io/rho/daemon#running-as-a-service";

const PROCESS_LABEL: &str = "Daemon";
const SOCKET_LABEL: &str = "Daemon socket";
const SERVICE_LABEL: &str = "Service";
const ENVIRONMENT_LABEL: &str = "Daemon environment";

/// Every daemon row's id and label, in the order the observation reports
/// them: what the probe's placeholders and failure rows show.
pub(super) const ROWS: [(DoctorCheckId, &str); 5] = [
    (DoctorCheckId::DaemonProcess, PROCESS_LABEL),
    (DoctorCheckId::DaemonSocket, SOCKET_LABEL),
    (DoctorCheckId::DaemonService, SERVICE_LABEL),
    (DoctorCheckId::DaemonEnvironment, ENVIRONMENT_LABEL),
    (
        DoctorCheckId::DaemonCredentialStore,
        super::daemon_store::LABEL,
    ),
];

/// Whether a daemon answers on this rho home's socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonProcess {
    NotRunning,
    /// It completed the handshake, so its protocol is compatible.
    Running {
        pid: u32,
        version: String,
        protocol: ProtocolVersion,
    },
    /// It refused the handshake over its protocol. `recorded` is the pid and
    /// rho version its info file names, when it wrote one.
    Incompatible {
        mismatch: VersionMismatch,
        recorded: Option<(u32, String)>,
    },
    /// A daemon holds this home's lock but listens on `socket`, not on
    /// `expected`, the socket this process resolves: it started with another
    /// `XDG_RUNTIME_DIR`, so no client started from here can reach it.
    OtherSocket {
        pid: u32,
        socket: String,
        expected: String,
    },
    /// Something is there but did not complete the handshake. The text says
    /// why.
    Unanswered(String),
}

/// What `rho doctor` found where the daemon listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonSocket {
    /// No socket: no daemon has started for this home, or it removed it.
    Missing {
        socket: String,
    },
    Private {
        socket: String,
    },
    /// The socket or its directory is open to someone else or is not what it
    /// should be. The text names the path and the fix.
    Exposed(String),
    /// It could not be looked at. The text says why.
    Unknown(String),
}

/// Whether the service `docs/daemon.md` documents is installed. The daemon
/// has no unit of its own: every client starts it on demand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonService {
    NotInstalled,
    /// The systemd user unit exists; `enabled` when a target wants it.
    Systemd {
        enabled: bool,
    },
    /// The LaunchAgent exists. Only `launchctl` knows whether it is loaded.
    Launchd,
    /// It could not be looked for. The text says why.
    Unknown(String),
}

/// How the daemon's own environment differs from this shell's, by name only:
/// values can be secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonEnvironment {
    NotRunning,
    /// The variables that differ among those the daemon reads from its own
    /// environment. `acts` holds those that change what the daemon does for
    /// this shell (network and daemon settings); `machine` the rest.
    Compared {
        acts: Vec<String>,
        machine: Vec<String>,
    },
    /// The daemon's environment cannot be read. The text says why.
    Unknown(String),
}

/// The row saying whether the daemon runs, with its pid, rho version and
/// protocol. A daemon this `rho` cannot reach or talk to fails, unless it is
/// an older one nobody is using, which the next client replaces by itself. A
/// rho version other than `client_version`, this `rho`'s, only warns, since
/// the protocol matches.
pub(crate) fn process_check(process: &DaemonProcess, client_version: &str) -> DoctorCheck {
    let row = |status, summary: String| {
        DoctorCheck::new(DoctorCheckId::DaemonProcess, PROCESS_LABEL, status, summary)
    };
    match process {
        DaemonProcess::NotRunning => row(DoctorStatus::Info, "not running".into()).with_hint(
            "the first client that needs it starts it; `rho daemon start` starts it now",
        ),
        DaemonProcess::Unanswered(reason) => row(DoctorStatus::Warn, "did not answer".into())
            .with_hint(format!("{reason}; run `rho daemon status` to ask it again")),
        DaemonProcess::Incompatible { mismatch, recorded } => {
            let who = match recorded {
                Some((pid, version)) => format!("pid {pid}, version {version}, "),
                None => String::new(),
            };
            // What a client's connect checks before replacing a daemon.
            let replaceable = mismatch.fix == VersionFix::RestartDaemon
                && mismatch.restart.is_some_and(RestartCost::is_free);
            let (status, hint) = if replaceable {
                (
                    DoctorStatus::Warn,
                    "older than this rho and unused, so the next client replaces it; `rho daemon restart` does it now".to_owned(),
                )
            } else {
                (DoctorStatus::Fail, mismatch.to_string())
            };
            row(
                status,
                format!(
                    "{who}protocol {} incompatible with this rho's {}",
                    mismatch.daemon, mismatch.client
                ),
            )
            .with_hint(hint)
        }
        DaemonProcess::OtherSocket {
            pid,
            socket,
            expected,
        } => row(
            DoctorStatus::Fail,
            format!("pid {pid} listens on another socket"),
        )
        .with_hint(format!(
            "it listens on {socket}, not {expected}: this shell has another XDG_RUNTIME_DIR than the one the daemon started with; set it to match"
        )),
        DaemonProcess::Running {
            pid,
            version,
            protocol,
        } => {
            let summary = format!("running, pid {pid}, version {version}, protocol {protocol}");
            if version == client_version {
                row(DoctorStatus::Ok, summary).with_hint("same version and protocol as this rho")
            } else if crate::update::version_is_newer(client_version, version) {
                row(DoctorStatus::Warn, summary).with_hint(format!(
                    "older than this rho ({client_version}); run `rho daemon restart` to replace it"
                ))
            } else {
                row(DoctorStatus::Warn, summary).with_hint(format!(
                    "not this rho's version ({client_version}); update this rho, or restart it if it was updated while it ran"
                ))
            }
        }
    }
}

/// The row for the socket's permissions. A socket or directory someone else
/// can reach fails: the socket is all that keeps other users from the
/// daemon, and a client refuses to start one in a directory that is not
/// private.
pub(crate) fn socket_check(socket: &DaemonSocket) -> DoctorCheck {
    let row = |status, summary: &str, hint: &str| {
        DoctorCheck::new(DoctorCheckId::DaemonSocket, SOCKET_LABEL, status, summary).with_hint(hint)
    };
    match socket {
        DaemonSocket::Missing { socket } => row(
            DoctorStatus::Info,
            "no socket",
            &format!("the daemon creates {socket} when it starts"),
        ),
        DaemonSocket::Private { socket } => row(
            DoctorStatus::Ok,
            "private",
            &format!("{socket}, reachable only by you"),
        ),
        DaemonSocket::Exposed(problem) => row(DoctorStatus::Fail, "not private", problem),
        DaemonSocket::Unknown(reason) => row(DoctorStatus::Warn, "unknown", reason),
    }
}

/// The row for the always-on service. Missing is not an issue, as nothing
/// needs it; a unit installed but never enabled warns, since it will not
/// start at the next login.
pub(crate) fn service_check(service: &DaemonService) -> DoctorCheck {
    let row = |status, summary: String, hint: String| {
        DoctorCheck::new(DoctorCheckId::DaemonService, SERVICE_LABEL, status, summary)
            .with_hint(hint)
    };
    match service {
        DaemonService::NotInstalled => row(
            DoctorStatus::Info,
            "not installed".into(),
            format!(
                "the daemon needs none, clients start it on demand; to keep `rho mcp serve --http` always on see {SERVICE_DOCS}"
            ),
        ),
        DaemonService::Systemd { enabled: true } => row(
            DoctorStatus::Ok,
            format!("{SYSTEMD_UNIT} installed and enabled"),
            format!("`systemctl --user status {SYSTEMD_UNIT}` shows whether it runs"),
        ),
        DaemonService::Systemd { enabled: false } => row(
            DoctorStatus::Warn,
            format!("{SYSTEMD_UNIT} installed, not enabled"),
            format!("run `systemctl --user enable --now {SYSTEMD_UNIT}` to start it at every login"),
        ),
        DaemonService::Launchd => row(
            DoctorStatus::Ok,
            format!("{LAUNCHD_LABEL} installed"),
            format!(
                "launchd loads it at login; `launchctl print gui/$(id -u)/{LAUNCHD_LABEL}` shows whether it is loaded now"
            ),
        ),
        DaemonService::Unknown(reason) => {
            row(DoctorStatus::Warn, "unknown".into(), reason.clone())
        }
    }
}

/// The row for the daemon's environment. Network and daemon settings that
/// differ warn: what this shell sets is not what the daemon uses. Machine
/// variables such as `PATH` differ between shells all the time, and sessions
/// take the client's, so those alone are informational.
pub(crate) fn environment_check(environment: &DaemonEnvironment) -> DoctorCheck {
    let row = |status, summary: String, hint: &str| {
        DoctorCheck::new(
            DoctorCheckId::DaemonEnvironment,
            ENVIRONMENT_LABEL,
            status,
            summary,
        )
        .with_hint(hint)
    };
    match environment {
        DaemonEnvironment::NotRunning => row(
            DoctorStatus::Info,
            "daemon not running".into(),
            "a daemon this shell starts takes its network and daemon settings from here",
        ),
        DaemonEnvironment::Unknown(reason) => row(
            DoctorStatus::Info,
            "unknown".into(),
            &format!(
                "{reason}; the daemon keeps the environment it started with, so `rho daemon stop` and a client run from this shell give it this one"
            ),
        ),
        DaemonEnvironment::Compared { acts, machine } => {
            let differing: Vec<&str> = acts.iter().chain(machine).map(String::as_str).collect();
            if differing.is_empty() {
                return row(
                    DoctorStatus::Ok,
                    "same as this shell".into(),
                    "compared the variables the daemon reads from its own environment",
                );
            }
            let summary = format!("differs in {}", differing.join(", "));
            if acts.is_empty() {
                row(
                    DoctorStatus::Info,
                    summary,
                    "sessions use their client's values, so this rarely matters; the daemon's own lookups keep the values it started with",
                )
            } else {
                row(
                    DoctorStatus::Warn,
                    summary,
                    "the daemon keeps the values it started with, so what this shell sets does not reach it; run `rho daemon stop`, then any client from this shell starts it with these",
                )
            }
        }
    }
}

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
