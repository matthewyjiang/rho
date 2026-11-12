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
    io,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// How long a client waits for systemd to move the daemon once it accepted
/// the scope.
//
// Tripwire. The call returns once systemd queued the scope's start job, not
// once it ran. Measured on a loaded 12-core host, 15 runs: the move landed
// 1.5 ms after the call returned, 30 ms at most. A daemon serves no session
// before a client connects, so the wait only keeps the processes its first
// session starts out of the client's unit.
const MOVE_TIMEOUT: Duration = Duration::from_secs(1);

const MOVE_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Moves the daemon running as `pid` into a scope of its own, beside the
/// unit this process runs in, when that unit is the user's systemd
/// manager's. A daemon that cannot be moved keeps running where it
/// started, as it did before; why is logged.
pub(super) fn leave_caller_unit(pid: u32, home: &Path) {
    let Ok(cgroup) = std::fs::read_to_string("/proc/self/cgroup") else {
        return;
    };
    let Some(slice) = caller_slice(&cgroup) else {
        return;
    };
    let unit = format!("rho-daemon-{pid}.scope");
    match start_scope(&unit, slice, pid, home).and_then(|()| wait_until_moved(pid, &unit)) {
        Ok(()) => tracing::debug!(%unit, slice, "moved the rho daemon into a scope of its own"),
        Err(error) => tracing::warn!(
            %unit,
            slice,
            %error,
            "could not move the rho daemon out of this process's systemd unit; stopping that unit stops the daemon"
        ),
    }
}

/// The slice holding the unit this process runs in, when the user's
/// systemd manager runs it, from the contents of `/proc/self/cgroup`. On
/// cgroup v2 that is a `0::` line whose path runs through
/// `user@<uid>.service`, then slices, then the unit, then any control
/// groups the unit made of its own.
fn caller_slice(proc_self_cgroup: &str) -> Option<&str> {
    let path = proc_self_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?;
    let components = path
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
    let description = format!("rho daemon for {}", home.display());
    let output = Command::new("busctl")
        .args([
            "--user",
            "--timeout=5",
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
            &pid.to_string(),
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
        ])
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "busctl {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Waits until `pid` runs in `unit`, up to [`MOVE_TIMEOUT`].
fn wait_until_moved(pid: u32, unit: &str) -> io::Result<()> {
    let started = Instant::now();
    let cgroup = format!("/proc/{pid}/cgroup");
    loop {
        let current = std::fs::read_to_string(&cgroup)?;
        if current
            .lines()
            .filter_map(|line| line.strip_prefix("0::"))
            .any(|path| path.rsplit('/').next() == Some(unit))
        {
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
