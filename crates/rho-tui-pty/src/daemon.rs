//! The daemon a sandbox's rho processes start: where it logs and listens,
//! and stopping it when the sandbox drops, so it never outlives the test.
//!
//! This mirrors rho's daemon layout and liveness rule rather than calling
//! them, because a test-harness crate cannot depend on rho's private
//! `daemon` module. Keep it in step with `crates/rho/src/daemon/paths.rs`
//! (`DaemonPaths::resolve`, ~95-117: lock and record under
//! `<rho home>/daemon/`, log at `logs/daemon.log`, socket at
//! `$XDG_RUNTIME_DIR/rho/<home id>/daemon.sock`) and `lock.rs` (`is_held`,
//! a non-blocking shared `flock` the daemon's exclusive one refuses, and
//! `read_record`).
//!
//! The daemon holds that lock until its run ends, just before it exits, and
//! the kernel releases it however the daemon dies, so the lock is what says
//! a daemon runs. Its pid is not:
//! `kill(pid, 0)` still finds a zombie nobody reaps (a container without a
//! reaping init), a pid the kernel gave to another process, and the pid in
//! a record a SIGKILLed daemon left behind.

use std::{fs, path::PathBuf};

#[cfg(unix)]
use std::{
    path::Path,
    time::{Duration, Instant},
};

use crate::env::IsolatedHome;

/// How long [`stop`] waits for the daemon to release its lock after
/// SIGTERM, and again after SIGKILL.
//
// Same budget as `rho daemon stop` (`DAEMON_STOP_TIMEOUT` in
// `crates/rho/src/app/daemon_cli.rs`), which waits for the same release: it
// covers `DRAIN_TIMEOUT` (4 s) while running turns save, plus the exit
// measured there at max 44 ms with all 24 cores saturated. After SIGKILL
// the daemon has nothing left to do, so the same budget only waits out a
// loaded host.
#[cfg(unix)]
const DAEMON_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`stop`] waits for a daemon that holds the lock to name itself
/// in its record.
//
// Same budget as rho's `DAEMON_START_TIMEOUT` (`crates/rho/src/daemon/spawn.rs`):
// a daemon takes the lock, then `bind` writes the record
// (`crates/rho/src/daemon/server.rs` ~73) before it accepts a connection,
// and exec to the first accepted connection measured max 14 ms with all 24
// cores saturated. It also outlasts a daemon that removed its record while
// draining (server.rs ~180), which releases the lock within `DRAIN_TIMEOUT`
// (4 s).
#[cfg(unix)]
const DAEMON_RECORD_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between checks while [`stop`] waits: the same as rho's
/// `LOCK_POLL_INTERVAL`. Each check is a `flock` probe, a record read, or a
/// `waitpid`; nothing tells a process that is not the daemon's parent when
/// it exits.
#[cfg(unix)]
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(10);

impl IsolatedHome {
    /// The daemon's log so far, empty before it starts.
    pub fn daemon_log(&self) -> String {
        fs::read_to_string(self.rho_home().join("logs/daemon.log")).unwrap_or_default()
    }

    /// The socket the sandbox's daemon listens on, once one does. It is
    /// `run/rho/<id>/daemon.sock`, and the sandbox's one rho home has one
    /// id, which rho derives from the home's canonical path.
    pub fn daemon_socket(&self) -> Option<PathBuf> {
        fs::read_dir(self.path().join("run/rho"))
            .ok()?
            .flatten()
            .map(|runtime_dir| runtime_dir.path().join("daemon.sock"))
            .find(|socket| socket.exists())
    }

    /// The pid in the daemon's record (`daemon/daemon.json` in the rho
    /// home), which a daemon writes once its socket is bound and removes
    /// once it stops serving. A daemon that was killed leaves its record
    /// behind, so the pid may name a process that is gone.
    pub fn daemon_pid(&self) -> Option<u32> {
        #[derive(serde::Deserialize)]
        struct DaemonRecord {
            pid: u32,
        }
        let record = fs::read(self.rho_home().join("daemon/daemon.json")).ok()?;
        serde_json::from_slice::<DaemonRecord>(&record)
            .ok()
            .map(|record| record.pid)
    }
}

/// What [`stop`] found holding the daemon lock.
#[cfg(unix)]
enum Holder {
    /// Nothing does, or nothing does any more.
    Released,
    /// The daemon with this pid, which took SIGTERM.
    Signalled(libc::pid_t),
}

/// Stops the daemon running from `home`, if any: SIGTERM, then SIGKILL if
/// it holds its lock past [`DAEMON_TEARDOWN_TIMEOUT`]. SIGTERM comes first
/// because the daemon's graceful stop also ends the tool processes its
/// sessions started, which SIGKILL would orphan. A running daemon this
/// process spawned is reaped, so it is gone, not a zombie, when this
/// returns; one a test already stopped, it must wait for itself.
///
/// Panics, unless the thread is already panicking, if the daemon outlives
/// both signals or never names itself.
#[cfg(unix)]
pub(crate) fn stop(home: &IsolatedHome) {
    let lock = home.rho_home().join("daemon/daemon.lock");
    let released = || !lock_held(&lock);
    let mut last_recorded = None;
    let holder = poll(DAEMON_RECORD_TIMEOUT, || {
        if released() {
            return Some(Holder::Released);
        }
        // A `pid: 0` record would signal this test's own process group.
        let pid = home
            .daemon_pid()
            .and_then(|pid| libc::pid_t::try_from(pid).ok())
            .filter(|&pid| pid > 0)?;
        last_recorded = Some(pid);
        // SAFETY: `kill` takes no pointers and has no memory-safety
        // preconditions.
        let signalled = unsafe { libc::kill(pid, libc::SIGTERM) } == 0;
        // Refused (ESRCH, or EPERM once another user's process has the pid)
        // while the lock is held, the record is one a killed daemon left,
        // which the daemon holding the lock replaces once it binds: read it
        // again.
        signalled.then_some(Holder::Signalled(pid))
    });
    let pid = match holder {
        Some(Holder::Released) => return,
        Some(Holder::Signalled(pid)) => pid,
        None => {
            fail_teardown(&format!(
                "a rho daemon held {} for DAEMON_RECORD_TIMEOUT ({DAEMON_RECORD_TIMEOUT:?}) without a record naming a running process (last recorded pid: {last_recorded:?}): it never finished starting, its record is stale, or it removed its record while draining and never finished",
                lock.display()
            ));
            return;
        }
    };
    if !poll_until(DAEMON_TEARDOWN_TIMEOUT, released) {
        // SAFETY: as above. `pid` stays the one that took SIGTERM: the
        // daemon removes its record while it drains.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        if !poll_until(DAEMON_TEARDOWN_TIMEOUT, released) {
            fail_teardown(&format!(
                "rho daemon (pid {pid}) held {} through SIGTERM and SIGKILL, waiting DAEMON_TEARDOWN_TIMEOUT ({DAEMON_TEARDOWN_TIMEOUT:?}) after each",
                lock.display()
            ));
            return;
        }
    }
    reap(pid);
}

/// Reaps `pid` if it is this process's child (a daemon a test spawned
/// itself), which would otherwise stay a zombie until the test binary
/// exits. Anyone else's pid fails `waitpid` with ECHILD at once.
///
/// It does not block: the daemon releases its lock when its run returns,
/// and the async runtime then shuts down before the process exits, which
/// a stuck blocking task could hold up for good.
#[cfg(unix)]
fn reap(pid: libc::pid_t) {
    // SAFETY: a null status pointer is allowed, and WNOHANG never blocks.
    // `waitpid` is 0 while our child runs, its pid once reaped, and -1
    // (ECHILD) for a process that is not our child.
    let finished = || unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) != 0 };
    if poll_until(DAEMON_TEARDOWN_TIMEOUT, finished) {
        return;
    }
    // SAFETY: as for `kill` in `stop`; this blocking `waitpid` returns once
    // SIGKILL ends our child.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
    fail_teardown(&format!(
        "rho daemon (pid {pid}) released its lock but had not exited DAEMON_TEARDOWN_TIMEOUT ({DAEMON_TEARDOWN_TIMEOUT:?}) later"
    ));
}

/// Whether a daemon holds `lock`, probed as rho's own `is_held` does: with
/// a non-blocking shared `flock` that the daemon's exclusive one refuses.
#[cfg(unix)]
fn lock_held(lock: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = fs::File::open(lock) else {
        return false;
    };
    // SAFETY: `flock` takes only a descriptor, which `file` keeps open.
    let probed = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    probed != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

/// Calls `attempt` every [`DAEMON_POLL_INTERVAL`] until it returns `Some`,
/// or `None` once `limit` passed.
#[cfg(unix)]
fn poll<T>(limit: Duration, mut attempt: impl FnMut() -> Option<T>) -> Option<T> {
    let started = Instant::now();
    loop {
        if let Some(value) = attempt() {
            return Some(value);
        }
        if started.elapsed() >= limit {
            return None;
        }
        std::thread::sleep(DAEMON_POLL_INTERVAL);
    }
}

/// Whether `done` holds within `limit`, checked as [`poll`] does.
#[cfg(unix)]
fn poll_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    poll(limit, || done().then_some(())).is_some()
}

/// Fails a sandbox teardown loudly, without turning a test that is already
/// panicking into an abort.
#[cfg(unix)]
fn fail_teardown(message: &str) {
    if std::thread::panicking() {
        eprintln!("{message}");
    } else {
        panic!("{message}");
    }
}
