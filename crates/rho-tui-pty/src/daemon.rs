//! The daemon a sandbox's rho processes start: where it logs and listens,
//! and stopping it when the sandbox drops, so it never outlives the test.
//!
//! This mirrors rho's daemon layout and liveness rule rather than calling
//! them, because a test-harness crate cannot depend on rho's private
//! `daemon` module. Keep it in step with `crates/rho/src/daemon/paths.rs`
//! (`DaemonPaths::resolve`: lock and record under `<rho home>/daemon/`, log
//! at `logs/daemon.log`), `lock.rs` (`is_held`, a non-blocking shared file
//! lock the daemon's exclusive one refuses, and `read_record`), and the
//! platform's endpoint and stop channel in `crates/rho/src/daemon/platform/`,
//! which `unix.rs` and `windows.rs` here mirror.
//!
//! The daemon holds that lock until its run ends, just before it exits, and
//! the kernel releases it however the daemon dies, so the lock is what says
//! a daemon runs. Its pid is not: a pid may name a zombie nobody reaps (a
//! container without a reaping init), a process the kernel gave the pid to
//! since, or the daemon of a record a killed one left behind.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::env::IsolatedHome;

#[cfg(unix)]
#[path = "daemon/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "daemon/windows.rs"]
mod platform;

/// How long [`stop`] waits for the daemon to release its lock after it was
/// asked to stop, and again after it was killed.
//
// Same budget as `rho daemon stop` (`DAEMON_STOP_TIMEOUT` in
// `crates/rho/src/app/daemon_cli.rs`), which waits for the same release: it
// covers `DRAIN_TIMEOUT` (4 s) while running turns save, plus the exit
// measured there at max 44 ms with all 24 cores saturated. Once killed
// the daemon has nothing left to do, so the same budget only waits out a
// loaded host.
const DAEMON_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`stop`] waits for a daemon that holds the lock to name itself
/// in its record.
//
// Same budget as rho's `DAEMON_START_TIMEOUT` (`crates/rho/src/daemon/spawn.rs`):
// a daemon takes the lock, then `bind` writes the record
// (`crates/rho/src/daemon/server.rs`) before it accepts a connection,
// and exec to the first accepted connection measured max 14 ms with all 24
// cores saturated. It also outlasts a daemon that removed its record while
// draining, which releases the lock within `DRAIN_TIMEOUT` (4 s).
const DAEMON_RECORD_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between checks while [`stop`] waits: the same as rho's
/// `LOCK_POLL_INTERVAL`. Each check is a lock probe, a record read, or a
/// wait for a child; nothing tells a process that is not the daemon's
/// parent when it exits.
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(10);

impl IsolatedHome {
    /// The daemon's log so far, empty before it starts.
    pub fn daemon_log(&self) -> String {
        fs::read_to_string(self.rho_home().join("logs/daemon.log")).unwrap_or_default()
    }

    /// What the sandbox's daemon listens on, once one does: a socket file
    /// on Unix, a named pipe on Windows.
    pub fn daemon_socket(&self) -> Option<PathBuf> {
        platform::socket(self)
    }

    /// The pid in the daemon's record, which a daemon writes once its
    /// socket is bound and removes once it stops serving. A daemon that was
    /// killed leaves its record behind, so the pid may name a process that
    /// is gone.
    pub fn daemon_pid(&self) -> Option<u32> {
        self.daemon_record()?.get("pid")?.as_u64()?.try_into().ok()
    }

    /// The daemon's record, `daemon/daemon.json` in the rho home.
    fn daemon_record(&self) -> Option<serde_json::Value> {
        let record = fs::read(self.rho_home().join("daemon/daemon.json")).ok()?;
        serde_json::from_slice(&record).ok()
    }
}

/// What [`stop`] found holding the daemon lock.
enum Holder {
    /// Nothing does, or nothing does any more.
    Released,
    /// The daemon with this pid, which was asked to stop.
    Asked(u32),
}

/// Stops the daemon running from `home`, if any: asks it to stop as
/// `rho daemon stop` asks a daemon it cannot talk to, then kills it if it
/// holds its lock past [`DAEMON_TEARDOWN_TIMEOUT`]. Asking comes first
/// because the daemon's graceful stop also ends the tool processes its
/// sessions started, which a kill would orphan. A running daemon this
/// process spawned is reaped, so it is gone, not a zombie, when this
/// returns; one a test already stopped, it must wait for itself.
///
/// Panics, unless the thread is already panicking, if the daemon outlives
/// both or never names itself.
pub(crate) fn stop(home: &IsolatedHome) {
    let lock = home.rho_home().join("daemon/daemon.lock");
    let released = || !lock_held(&lock);
    let mut last_recorded = None;
    let holder = poll(DAEMON_RECORD_TIMEOUT, || {
        if released() {
            return Some(Holder::Released);
        }
        // A `pid: 0` record would signal this test's own process group.
        let pid = home.daemon_pid().filter(|&pid| pid > 0)?;
        last_recorded = Some(pid);
        // Refused while the lock is held, the record is one a killed daemon
        // left, which the daemon holding the lock replaces once it binds:
        // read it again.
        platform::ask_to_stop(home, pid).then_some(Holder::Asked(pid))
    });
    let pid = match holder {
        Some(Holder::Released) => return,
        Some(Holder::Asked(pid)) => pid,
        None => {
            fail_teardown(&format!(
                "a rho daemon held {} for DAEMON_RECORD_TIMEOUT ({DAEMON_RECORD_TIMEOUT:?}) without a record naming a running daemon (last recorded pid: {last_recorded:?}): it never finished starting, its record is stale, or it removed its record while draining and never finished",
                lock.display()
            ));
            return;
        }
    };
    if !poll_until(DAEMON_TEARDOWN_TIMEOUT, released) {
        // `pid` stays the one that was asked: the daemon removes its record
        // while it drains.
        platform::kill(pid);
        if !poll_until(DAEMON_TEARDOWN_TIMEOUT, released) {
            fail_teardown(&format!(
                "rho daemon (pid {pid}) held {} through a stop request and a kill, waiting DAEMON_TEARDOWN_TIMEOUT ({DAEMON_TEARDOWN_TIMEOUT:?}) after each",
                lock.display()
            ));
            return;
        }
    }
    platform::reap(pid);
}

/// Whether a daemon holds `lock`, probed as rho's own `is_held` does: with
/// a non-blocking shared lock that the daemon's exclusive one refuses.
fn lock_held(lock: &Path) -> bool {
    let Ok(file) = fs::File::options().read(true).write(true).open(lock) else {
        return false;
    };
    // `fs2` reports contention as `WouldBlock` on Unix and as the raw
    // lock-violation error on Windows.
    fs2::FileExt::try_lock_shared(&file).is_err_and(|error| {
        error.kind() == std::io::ErrorKind::WouldBlock
            || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
    })
}

/// Calls `attempt` every [`DAEMON_POLL_INTERVAL`] until it returns `Some`,
/// or `None` once `limit` passed.
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
fn poll_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    poll(limit, || done().then_some(())).is_some()
}

/// Fails a sandbox teardown loudly, without turning a test that is already
/// panicking into an abort.
fn fail_teardown(message: &str) {
    if std::thread::panicking() {
        eprintln!("{message}");
    } else {
        panic!("{message}");
    }
}
