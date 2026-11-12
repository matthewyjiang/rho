use pretty_assertions::assert_eq;
use rho_protocol::{ProtocolVersion, VersionFix, VersionMismatch, PROTOCOL_VERSION};
use tempfile::TempDir;

use super::*;
use crate::daemon::test_support::RefusingListener;

/// Failure bound for a daemon stand-in to answer; no test waits it out.
const DEADLINE: Duration = Duration::from_secs(10);

fn paths(root: &TempDir) -> DaemonPaths {
    DaemonPaths::resolve(root.path(), |_| None).unwrap()
}

// Covers: with nothing listening, doctor reports the daemon as not running
// for both rows it asks the daemon for, and creates nothing: asking must
// never start a daemon or leave its directories behind.
// Owner: doctor daemon observation (socket, no daemon).
#[tokio::test]
async fn no_daemon_is_reported_as_not_running_and_nothing_is_created() {
    let root = TempDir::new().unwrap();
    let paths = paths(&root);

    let answer = ask(&paths, DEADLINE).await;

    assert_eq!(
        (answer, inspect_socket(&paths), paths.runtime_dir.exists()),
        (
            (DaemonProcess::NotRunning, DaemonStore::NotRunning),
            DaemonSocket::Missing {
                socket: paths.socket.display().to_string()
            },
            false
        )
    );
}

// Covers: a daemon that refuses the handshake over its protocol is reported
// as incompatible with the mismatch it sent, which says which side to change,
// rather than as down or as a generic connection error.
// Owner: doctor daemon observation (protocol handshake).
#[tokio::test]
async fn a_daemon_of_another_protocol_is_reported_as_incompatible() {
    let root = TempDir::new().unwrap();
    let paths = paths(&root);
    let mut listener = RefusingListener::bind(&paths);
    let mismatch = VersionMismatch {
        daemon: ProtocolVersion::new(0, 0),
        client: PROTOCOL_VERSION,
        fix: VersionFix::RestartDaemon,
        restart: None,
    };
    let daemon = tokio::spawn(async move { listener.refuse_next(mismatch).await });

    let answer = ask(&paths, DEADLINE).await;
    daemon.await.unwrap();

    assert_eq!(
        answer,
        (
            DaemonProcess::Incompatible {
                mismatch,
                recorded: None
            },
            DaemonStore::Unanswered(mismatch.to_string())
        )
    );
}

// Covers: a daemon that holds this home's lock but listens in another
// runtime directory is reported as on another socket, with the pid and both
// paths, instead of as not running: `rho daemon start` from here would fail,
// not start one.
// Owner: doctor daemon observation (lock and info file).
#[tokio::test]
async fn a_daemon_on_another_socket_is_not_reported_as_down() {
    let root = TempDir::new().unwrap();
    let paths = paths(&root);
    let _lock = daemon::acquire_lock(&paths).unwrap();
    // A daemon holding the lock with no record is starting or stopping.
    let starting = ask(&paths, DEADLINE).await.0;
    assert!(
        matches!(starting, DaemonProcess::Unanswered(_)),
        "{starting:?}"
    );
    std::fs::write(
        &paths.info_file,
        serde_json::json!({
            "pid": 4242,
            "version": "3.0.0",
            "protocol": PROTOCOL_VERSION,
            "started_at": 0,
            "socket": "/run/user/1000/rho/id/daemon.sock",
        })
        .to_string(),
    )
    .unwrap();

    let answer = ask(&paths, DEADLINE).await;

    assert_eq!(
        answer,
        (
            DaemonProcess::OtherSocket {
                pid: 4242,
                socket: "/run/user/1000/rho/id/daemon.sock".into(),
                expected: paths.socket.display().to_string(),
            },
            DaemonStore::Unanswered("it listens on another socket".into())
        )
    );
}

// Covers: a socket or runtime directory someone else can reach is reported
// as exposed, naming the path and mode, and a private pair is not.
// Owner: doctor daemon observation (filesystem permissions).
#[cfg(unix)]
#[tokio::test]
async fn a_socket_others_can_reach_is_reported_as_exposed() {
    use std::os::unix::fs::PermissionsExt;

    use crate::daemon::DaemonPathError;

    let root = TempDir::new().unwrap();
    let paths = paths(&root);
    let _listener = RefusingListener::bind(&paths);
    let chmod = |path: &Path, mode| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let socket = paths.socket.display().to_string();
    let exposed = |error: DaemonPathError| DaemonSocket::Exposed(error.to_string());

    chmod(&paths.socket, 0o600);
    let private = inspect_socket(&paths);
    chmod(&paths.socket, 0o666);
    let open_socket = inspect_socket(&paths);
    chmod(&paths.socket, 0o600);
    chmod(&paths.runtime_dir, 0o750);
    let open_dir = inspect_socket(&paths);

    assert_eq!(
        [private, open_socket, open_dir],
        [
            DaemonSocket::Private { socket },
            exposed(DaemonPathError::InsecureSocketMode {
                path: paths.socket.clone(),
                mode: 0o666
            }),
            exposed(DaemonPathError::InsecureMode {
                path: paths.runtime_dir.clone(),
                mode: 0o750
            }),
        ]
    );
}

// Covers: the documented systemd unit is found in the user's unit directory,
// under XDG_CONFIG_HOME when that is absolute, and counts as enabled only
// once a target's `.wants` directory links to it, as `systemctl --user
// enable` leaves it.
// Owner: doctor daemon observation (service files).
#[cfg(unix)]
#[test]
fn the_systemd_unit_is_installed_once_saved_and_enabled_once_wanted() {
    use crate::daemon::SYSTEMD_UNIT;
    let home = TempDir::new().unwrap();
    let default_units = home.path().join(".config/systemd/user");
    let xdg = home.path().join("xdg");
    let xdg_units = xdg.join("systemd/user");
    let find = |xdg: Option<&Path>| find_service(home.path(), xdg.map(Path::to_path_buf));

    let missing = find(None);
    std::fs::create_dir_all(&default_units).unwrap();
    std::fs::write(default_units.join(SYSTEMD_UNIT), "[Service]\n").unwrap();
    let saved = find(None);
    let elsewhere = find(Some(&xdg));
    let relative = find(Some(Path::new("relative")));
    let wants = xdg_units.join("default.target.wants");
    std::fs::create_dir_all(&wants).unwrap();
    std::fs::write(xdg_units.join(SYSTEMD_UNIT), "[Service]\n").unwrap();
    std::os::unix::fs::symlink(xdg_units.join(SYSTEMD_UNIT), wants.join(SYSTEMD_UNIT)).unwrap();
    let enabled = find(Some(&xdg));

    assert_eq!(
        [missing, saved, elsewhere, relative, enabled],
        [
            DaemonService::NotInstalled,
            DaemonService::Systemd { enabled: false },
            DaemonService::NotInstalled,
            DaemonService::Systemd { enabled: false },
            DaemonService::Systemd { enabled: true },
        ]
    );
}

// Covers: the LaunchAgent the docs have the user save is found by its label.
// Owner: doctor daemon observation (service files).
#[test]
fn the_launch_agent_is_installed_once_saved() {
    let home = TempDir::new().unwrap();
    let agents = home.path().join("Library/LaunchAgents");

    let missing = find_service(home.path(), None);
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("dev.rho.mcp.plist"), "").unwrap();
    let saved = find_service(home.path(), None);

    assert_eq!(
        [missing, saved],
        [DaemonService::NotInstalled, DaemonService::Launchd]
    );
}

// Covers: the daemon's environment as /proc shows it is compared with this
// shell's by name: a network or daemon setting that differs, or that only
// one side has, is one that changes what the daemon does; a machine variable
// is not; and a value containing `=` is not cut short.
// Owner: doctor daemon observation (environment comparison).
#[cfg(target_os = "linux")]
#[test]
fn a_daemons_environment_is_compared_by_what_it_changes() {
    let daemon = parse_environ(
        b"PATH=/usr/bin\0HTTPS_PROXY=http://proxy:3128/?a=b\0LANG=C.UTF-8\0RHO_LOG=debug\0",
    );
    let client = |name: &str| -> Option<OsString> {
        match name {
            "PATH" => Some("/home/dev/bin:/usr/bin".into()),
            "HTTPS_PROXY" => Some("http://proxy:3128/?a=b".into()),
            "LANG" => Some("C.UTF-8".into()),
            "NO_PROXY" => Some("localhost".into()),
            _ => None,
        }
    };

    assert_eq!(
        compare_environments(&daemon, client),
        DaemonEnvironment::Compared {
            acts: vec!["NO_PROXY".into(), "RHO_LOG".into()],
            machine: vec!["PATH".into()],
        }
    );
}

// Covers: the daemon probe's rows replace exactly its placeholders, so
// `/doctor` never keeps spinning on a daemon row nothing fills in.
// Owner: doctor daemon observation (pure)
#[test]
fn rows_fill_the_probe_placeholders() {
    let ids = |rows: Vec<DoctorCheck>| rows.into_iter().map(|row| row.id).collect::<Vec<_>>();

    let observed = rows(
        &DaemonProcess::NotRunning,
        &DaemonSocket::Unknown("unread".into()),
        &DaemonService::NotInstalled,
        &DaemonEnvironment::NotRunning,
        &DaemonStore::NotRunning,
    );

    assert_eq!(
        ids(observed),
        ids(super::super::probes::placeholder_checks(
            &super::super::DoctorProbeId::Daemon
        ))
    );
}
