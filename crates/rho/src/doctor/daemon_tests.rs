use pretty_assertions::assert_eq;
use rho_protocol::{RestartCost, VersionFix, PROTOCOL_VERSION};

use super::*;

const CLIENT: &str = "3.1.0";

fn running(version: &str) -> DaemonProcess {
    DaemonProcess::Running {
        pid: 4242,
        version: version.into(),
        protocol: PROTOCOL_VERSION,
    }
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

// Covers: each daemon row's severity follows what was observed. Only what
// stops this `rho` from using the daemon fails: an incompatible protocol,
// unless the daemon is an unused older one the next client replaces, a
// daemon listening where this shell does not look, and a socket someone
// else can reach. A daemon of another rho version, a unit
// never enabled, and a network or daemon setting this shell sets that the
// daemon does not have warn. A daemon that is not running, a service nobody
// installed, and machine variables such as PATH that differ between shells
// never count as issues. Every row carries a hint.
// Owner: doctor daemon rows (pure)
#[test]
fn row_severity_follows_what_was_observed() {
    use DoctorStatus::{Fail, Info, Ok, Warn};

    let incompatible = |fix, restart: Option<(u64, u64)>| DaemonProcess::Incompatible {
        mismatch: VersionMismatch {
            daemon: ProtocolVersion::new(0, 0),
            client: PROTOCOL_VERSION,
            fix,
            restart: restart.map(|(in_progress, clients)| RestartCost {
                in_progress,
                clients,
            }),
        },
        recorded: Some((4242, "2.0.0".into())),
    };
    let elsewhere = DaemonProcess::OtherSocket {
        pid: 4242,
        socket: "/run/user/1000/rho/id/daemon.sock".into(),
        expected: "/home/dev/.rho/run/daemon.sock".into(),
    };
    let compared = |acts: &[&str], machine: &[&str]| DaemonEnvironment::Compared {
        acts: names(acts),
        machine: names(machine),
    };
    let rows = [
        (process_check(&DaemonProcess::NotRunning, CLIENT), Info),
        (process_check(&running(CLIENT), CLIENT), Ok),
        (process_check(&running("3.0.9"), CLIENT), Warn),
        (process_check(&running("3.2.0"), CLIENT), Warn),
        (
            process_check(&incompatible(VersionFix::RestartDaemon, None), CLIENT),
            Fail,
        ),
        (
            process_check(
                &incompatible(VersionFix::RestartDaemon, Some((1, 0))),
                CLIENT,
            ),
            Fail,
        ),
        (
            process_check(
                &incompatible(VersionFix::RestartDaemon, Some((0, 0))),
                CLIENT,
            ),
            Warn,
        ),
        (
            process_check(
                &incompatible(VersionFix::UpdateClient, Some((0, 0))),
                CLIENT,
            ),
            Fail,
        ),
        (process_check(&elsewhere, CLIENT), Fail),
        (
            process_check(&DaemonProcess::Unanswered("closed".into()), CLIENT),
            Warn,
        ),
        (
            socket_check(&DaemonSocket::Missing {
                socket: "/run/daemon.sock".into(),
            }),
            Info,
        ),
        (
            socket_check(&DaemonSocket::Private {
                socket: "/run/daemon.sock".into(),
            }),
            Ok,
        ),
        (
            socket_check(&DaemonSocket::Exposed("mode 0666".into())),
            Fail,
        ),
        (socket_check(&DaemonSocket::Unknown("denied".into())), Warn),
        (service_check(&DaemonService::NotInstalled), Info),
        (service_check(&DaemonService::Systemd { enabled: true }), Ok),
        (
            service_check(&DaemonService::Systemd { enabled: false }),
            Warn,
        ),
        (service_check(&DaemonService::Launchd), Ok),
        (
            service_check(&DaemonService::Unknown("no home".into())),
            Warn,
        ),
        (environment_check(&DaemonEnvironment::NotRunning), Info),
        (environment_check(&compared(&[], &[])), Ok),
        (environment_check(&compared(&[], &["PATH"])), Info),
        (
            environment_check(&compared(&["HTTPS_PROXY"], &["PATH"])),
            Warn,
        ),
        (
            environment_check(&DaemonEnvironment::Unknown("no /proc".into())),
            Info,
        ),
    ];

    let unhinted: Vec<_> = rows
        .iter()
        .filter(|(row, _)| row.hint.as_deref().unwrap_or_default().is_empty())
        .map(|(row, _)| row.summary.clone())
        .collect();
    assert_eq!(unhinted, Vec::<String>::new());
    let (rows, expected): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
    let actual: Vec<_> = rows
        .iter()
        .map(|row| (row.label.as_str(), row.summary.as_str(), row.status))
        .collect();
    let expected: Vec<_> = rows
        .iter()
        .zip(expected)
        .map(|(row, status)| (row.label.as_str(), row.summary.as_str(), status))
        .collect();
    assert_eq!(actual, expected);
}
