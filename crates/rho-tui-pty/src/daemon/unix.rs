//! The Unix daemon as the harness reaches it: a socket file under
//! `$XDG_RUNTIME_DIR`, and signals.

use std::{fs, path::PathBuf};

use super::{fail_teardown, poll_until, DAEMON_TEARDOWN_TIMEOUT};
use crate::env::IsolatedHome;

/// The socket file, `run/rho/<id>/daemon.sock`: the sandbox's one rho home
/// has one id, which rho derives from the home's canonical path.
pub(super) fn socket(home: &IsolatedHome) -> Option<PathBuf> {
    fs::read_dir(home.path().join("run/rho"))
        .ok()?
        .flatten()
        .map(|runtime_dir| runtime_dir.path().join("daemon.sock"))
        .find(|socket| socket.exists())
}

fn pid_of(pid: u32) -> Option<libc::pid_t> {
    libc::pid_t::try_from(pid).ok()
}

/// Sends the recorded daemon SIGTERM. Refused (ESRCH, or EPERM once another
/// user's process has the pid) when the record is stale.
pub(super) fn ask_to_stop(_home: &IsolatedHome, pid: u32) -> bool {
    // SAFETY: `kill` takes no pointers and has no memory-safety
    // preconditions.
    pid_of(pid).is_some_and(|pid| unsafe { libc::kill(pid, libc::SIGTERM) } == 0)
}

/// Sends `pid` SIGKILL.
pub(super) fn kill(pid: u32) {
    if let Some(pid) = pid_of(pid) {
        // SAFETY: as in `ask_to_stop`.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// Reaps `pid` if it is this process's child (a daemon a test spawned
/// itself), which would otherwise stay a zombie until the test binary
/// exits. Anyone else's pid fails `waitpid` with ECHILD at once.
///
/// It does not block: the daemon releases its lock when its run returns,
/// and the async runtime then shuts down before the process exits, which
/// a stuck blocking task could hold up for good.
pub(super) fn reap(pid: u32) {
    let Some(pid) = pid_of(pid) else {
        return;
    };
    // SAFETY: a null status pointer is allowed, and WNOHANG never blocks.
    // `waitpid` is 0 while our child runs, its pid once reaped, and -1
    // (ECHILD) for a process that is not our child.
    let finished = || unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) != 0 };
    if poll_until(DAEMON_TEARDOWN_TIMEOUT, finished) {
        return;
    }
    // SAFETY: as for `kill`; this blocking `waitpid` returns once SIGKILL
    // ends our child.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
    fail_teardown(&format!(
        "rho daemon (pid {pid}) released its lock but had not exited DAEMON_TEARDOWN_TIMEOUT ({DAEMON_TEARDOWN_TIMEOUT:?}) later"
    ));
}
