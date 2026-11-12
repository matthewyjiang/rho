//! Moving a daemon a client starts out of the client's systemd unit.
//!
//! A new process group does not leave a control group. A client the user's
//! systemd manager runs in a unit of its own, such as the transient scope
//! T3 Code starts each agent in, or a terminal's scope, would otherwise
//! leave the daemon in that unit: stopping the unit stops the daemon and
//! every session it serves, and the unit is charged for the memory of all
//! of them and their tools (#1685). So the daemon moves into a scope of its
//! own in the same slice, which keeps whatever limits the slice sets.
//!
//! Only a client under the user's manager (`user@<uid>.service`) is moved:
//! the manager cannot take a process from a unit of the system manager,
//! such as a login's `session-<n>.scope`. Where `/proc/self/cgroup` does
//! not exist, as on macOS, nothing is moved.

use std::{
    io::{self, Write},
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use super::service::run_manager_command;
use crate::daemon::paths::DaemonPaths;

/// How long the user's manager may take to answer the request for the
/// scope.
//
// Tripwire. Measured on a loaded 12-core host, 15 runs: the call took
// 4.3 to 5.6 ms. A manager that takes a second is wedged, and the client is
// waiting on it before it can reach the daemon at all.
const SCOPE_CALL_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a client waits for systemd to move the daemon once it accepted
/// the scope.
//
// Tripwire. The call returns once systemd queued the scope's start job, not
// once it ran. Measured on the same host and runs: the move landed 1.5 ms
// after the call returned, 30 ms at most. A daemon serves no session
// before a client connects, so the wait only keeps the processes its first
// session starts out of the client's unit.
const MOVE_TIMEOUT: Duration = Duration::from_secs(1);

const MOVE_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Moves the daemon running as `pid` into a scope of its own, beside the
/// unit this process runs in, when that unit is the user's systemd
/// manager's. Blocks until systemd moved it, up to [`SCOPE_CALL_TIMEOUT`]
/// plus [`MOVE_TIMEOUT`]. A daemon whose move is not confirmed keeps
/// running, likely where it started, as it did before; why goes to the
/// daemon's log, which a client that logs nothing itself, such as `rho acp`
/// under an editor host, still leaves behind.
///
/// `pid` must be a child this process has not reaped yet, so no other
/// process can have taken the pid by the time systemd moves it.
pub(super) fn leave_caller_unit(pid: u32, paths: &DaemonPaths) {
    let Ok(cgroup) = std::fs::read_to_string("/proc/self/cgroup") else {
        return;
    };
    let Some(slice) = caller_slice(&cgroup) else {
        return;
    };
    let unit = format!("rho-daemon-{pid}.scope");
    let moved =
        start_scope(&unit, slice, pid, &paths.home).and_then(|()| wait_until_moved(pid, &unit));
    let Err(error) = moved else {
        tracing::debug!(%unit, slice, "moved the rho daemon into a scope of its own");
        return;
    };
    let warning = format!(
        "could not confirm the rho daemon (pid {pid}) moved into {unit} in {slice}; \
         stopping the systemd unit of the client that started it may stop it: {error}"
    );
    tracing::warn!("{warning}");
    let line = format!(
        "{}  WARN {}: {warning}\n",
        humantime::format_rfc3339_micros(SystemTime::now()),
        module_path!()
    );
    if let Err(error) = paths
        .open_log_file()
        .and_then(|mut log| log.write_all(line.as_bytes()))
    {
        tracing::warn!(%error, log_file = %paths.log_file.display(), "could not write to the rho daemon log");
    }
}

/// The path of the cgroup v2 hierarchy in the contents of a
/// `/proc/<pid>/cgroup` file: the `0::` line's.
fn unified_path(proc_cgroup: &str) -> Option<&str> {
    proc_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
}

/// The slice holding the unit this process runs in, when the user's
/// systemd manager runs it, from the contents of `/proc/self/cgroup`. On
/// cgroup v2 that path runs through `user@<uid>.service`, then slices, then
/// the unit, then any control groups the unit made of its own.
fn caller_slice(proc_self_cgroup: &str) -> Option<&str> {
    let components = unified_path(proc_self_cgroup)?
        .split('/')
        .skip_while(|component| !is_user_manager(component))
        .skip(1);
    let mut slice = None;
    for component in components {
        if !component.ends_with(".slice") {
            // The first component that is no slice is the unit.
            return slice;
        }
        slice = Some(component);
    }
    None
}

fn is_user_manager(component: &str) -> bool {
    component
        .strip_prefix("user@")
        .and_then(|rest| rest.strip_suffix(".service"))
        .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Asks the user's manager to start `unit`, a scope in `slice` holding
/// `pid`, which it removes once the daemon exits.
fn start_scope(unit: &str, slice: &str, pid: u32, home: &Path) -> io::Result<()> {
    let timeout = format!("--timeout={}", SCOPE_CALL_TIMEOUT.as_secs());
    let pid = pid.to_string();
    let description = format!("rho daemon for {}", home.display());
    run_manager_command(
        "busctl",
        &[
            "--user",
            &timeout,
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            unit,
            "fail",
            "4",
            "PIDs",
            "au",
            "1",
            &pid,
            "Slice",
            "s",
            slice,
            "CollectMode",
            "s",
            "inactive-or-failed",
            "Description",
            "s",
            &description,
            "0",
        ],
    )
}

/// Waits until `pid` runs in `unit`, up to [`MOVE_TIMEOUT`].
fn wait_until_moved(pid: u32, unit: &str) -> io::Result<()> {
    let started = Instant::now();
    let cgroup = format!("/proc/{pid}/cgroup");
    loop {
        let current = std::fs::read_to_string(&cgroup)?;
        if unified_path(&current).and_then(|path| path.rsplit('/').next()) == Some(unit) {
            return Ok(());
        }
        let elapsed = started.elapsed();
        if elapsed >= MOVE_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("not in {unit} after MOVE_TIMEOUT ({MOVE_TIMEOUT:?}; waited {elapsed:?})"),
            ));
        }
        std::thread::sleep(MOVE_POLL_INTERVAL);
    }
}

#[cfg(test)]
#[path = "user_scope_tests.rs"]
mod tests;
