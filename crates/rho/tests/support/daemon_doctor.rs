//! `rho doctor`'s daemon rows through the real binary (#1674).

use std::process::Stdio;

use pretty_assertions::assert_eq;
use serde_json::Value;

use super::sessions::Sandbox;

/// The `(kind, status)` of the daemon rows `rho doctor --json` prints with
/// `env` added to the sandbox's environment, in report order, and the
/// summary of the `daemon_process` row. The credential store row is left
/// out: whether an OS store answers depends on the machine running the test
/// (see [`super::headless`]).
fn daemon_rows(sandbox: &Sandbox, env: &[(&str, &str)]) -> (Vec<(String, String)>, String) {
    let output = sandbox
        .rho("fixed")
        .envs(env.iter().copied())
        .args(["doctor", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {output:?}"));
    let rows: Vec<&Value> = report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|section| section["checks"].as_array().unwrap())
        .filter(|check| {
            let kind = check["id"]["kind"].as_str().unwrap();
            kind.starts_with("daemon_") && kind != "daemon_credential_store"
        })
        .collect();
    assert!(
        rows.iter().all(|row| row["hint"].is_string()),
        "every daemon row has a hint: {rows:?}"
    );
    let text = |value: &Value| value.as_str().unwrap().to_owned();
    let process = rows
        .iter()
        .find(|row| row["id"]["kind"] == "daemon_process")
        .map(|row| text(&row["summary"]))
        .unwrap();
    let statuses = rows
        .iter()
        .map(|row| (text(&row["id"]["kind"]), text(&row["status"])))
        .collect();
    (statuses, process)
}

fn rows(statuses: [&str; 4]) -> Vec<(String, String)> {
    [
        "daemon_process",
        "daemon_socket",
        "daemon_service",
        "daemon_environment",
    ]
    .into_iter()
    .zip(statuses)
    .map(|(kind, status)| (kind.to_owned(), status.to_owned()))
    .collect()
}

/// Opens the daemon's socket to other users, where a file mode does that:
/// a named pipe's access control list is fixed when it is created.
fn open_socket_to_others(sandbox: &Sandbox) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let socket = sandbox.isolated.daemon_socket().unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = sandbox;
    cfg!(unix)
}

// Covers: `rho doctor --json` reports every daemon check without starting a
// daemon; a running one is reported with its pid, version, and protocol
// and a private socket; a daemon setting this shell
// changed after the daemon started warns; and a socket opened to others
// fails.
// Owner: CLI process (doctor against a real daemon).
#[test]
fn doctor_reports_the_daemon_it_finds() {
    let sandbox = Sandbox::new();
    // Where the OS shows no process's environment the row says so.
    let (same, changed) = if cfg!(target_os = "linux") {
        ("ok", "warn")
    } else {
        ("info", "info")
    };

    let down = daemon_rows(&sandbox, &[]);
    let spawned = sandbox.isolated.daemon_socket();
    let started = sandbox
        .rho("fixed")
        .args(["daemon", "start"])
        .status()
        .unwrap();
    assert!(started.success());
    let pid = sandbox.isolated.daemon_pid().unwrap();
    let up = daemon_rows(&sandbox, &[]);
    let other_log = daemon_rows(&sandbox, &[("RHO_LOG", "rho=trace")]);
    let exposed = open_socket_to_others(&sandbox);
    let exposed_rows = exposed.then(|| daemon_rows(&sandbox, &[]).0);

    let running = format!(
        "running, pid {pid}, version {}, protocol {}",
        env!("CARGO_PKG_VERSION"),
        rho_protocol::PROTOCOL_VERSION
    );
    assert_eq!(
        (down, spawned, up, other_log.0, exposed_rows),
        (
            (
                rows(["info", "info", "info", "info"]),
                "not running".to_owned()
            ),
            None,
            (rows(["ok", "ok", "info", same]), running),
            rows(["ok", "ok", "info", changed]),
            exposed.then(|| rows(["ok", "fail", "info", same])),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
