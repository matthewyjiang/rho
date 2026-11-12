//! Restarting (#1778): Windows cannot replace a running process with
//! another program, and a file lock does not pass to a child, so the daemon
//! stops as for any stop and starts its binary as a new process.
//!
//! The new process cannot create the daemon's pipes while the old one has
//! any instance of them open, and a closed instance stays open until the
//! process that closed it is gone. So the old process tells the new one its
//! pid as the handoff token, and the new one waits for it to exit before it
//! takes the lock and creates the pipes.
//!
//! Between the two nothing listens. A frontend that reconnects finds no
//! daemon and starts one, as it would after a stop; the two then race for
//! the lock, one runs, and the other exits. `rho daemon restart` waits for
//! whichever answers (`connect_restarting`).

use std::{
    ffi::OsString,
    fs::File,
    io,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

use super::{endpoint::Listener, files::open_lock_file, process::detach};
use crate::daemon::{paths::DaemonPaths, server::Restarting};

/// How long a restarted daemon waits for the daemon that started it to
/// exit.
//
// Tripwire. That daemon has saved its turns and released its lock by then,
// and exits as soon as starting this one returned. Measured with the debug
// binary over 300 restarts each: idle, it had always exited before this
// one looked; with all 16 cores loaded it was still running 2 times, for
// under 3 µs. Hitting the limit means it hung while exiting.
const PREDECESSOR_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Stops this daemon, then starts `executable` with the arguments `args`
/// makes of this process's pid, the handoff token: the new process waits
/// for this one to exit, then takes the lock and creates the pipe itself.
/// `Ok` once it started, and this process should exit. An error comes once
/// the lock is released and the pipe closed, so the next client starts a
/// new daemon.
pub(in crate::daemon) fn relaunch(
    restarting: Restarting,
    executable: &Path,
    args: impl FnOnce(Option<String>) -> Vec<OsString>,
) -> io::Result<()> {
    let paths = restarting.paths.clone();
    restarting.release();
    tracing::info!(executable = %executable.display(), "starting the rho daemon again");
    let mut command = Command::new(executable);
    command
        .args(args(Some(std::process::id().to_string())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command, &paths);
    command.spawn().map(drop).inspect_err(|error| {
        tracing::error!(
            executable = %executable.display(),
            %error,
            "could not start the rho daemon again; exiting"
        );
    })
}

/// Waits for the daemon whose pid is `token` to exit, then opens the lock
/// file it released. The lock is the caller's to take, and only then the
/// pipe to create: a client that found no daemon meanwhile may have started
/// one, which then holds both.
pub(in crate::daemon) fn take_handoff(
    paths: &DaemonPaths,
    token: &str,
) -> io::Result<(File, Option<Listener>)> {
    let predecessor = token.parse::<u32>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("expected the pid of the daemon that restarted, got {token:?}: {error}"),
        )
    })?;
    wait_for_exit(predecessor)?;
    Ok((open_lock_file(&paths.lock)?, None))
}

/// Blocks until process `pid` has exited, which closes every handle it had.
fn wait_for_exit(pid: u32) -> io::Result<()> {
    // SAFETY: the call takes no pointers.
    let process = unsafe {
        OpenProcess(PROCESS_SYNCHRONIZE, /*bInheritHandle*/ 0, pid)
    };
    if process.is_null() {
        let error = io::Error::last_os_error();
        // No process has that pid: it exited already.
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Ok(())
        } else {
            Err(error)
        };
    }
    let started = Instant::now();
    // SAFETY: `process` is the open handle `OpenProcess` returned, closed
    // once, after the wait.
    let waited = unsafe {
        let waited = WaitForSingleObject(process, PREDECESSOR_EXIT_TIMEOUT.as_millis() as u32);
        let error = io::Error::last_os_error();
        CloseHandle(process);
        (waited, error)
    };
    let elapsed = started.elapsed();
    match waited {
        (WAIT_OBJECT_0, _) => {
            tracing::debug!(pid, ?elapsed, "the daemon that restarted has exited");
            Ok(())
        }
        (WAIT_TIMEOUT, _) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "the rho daemon that restarted (pid {pid}) did not exit within PREDECESSOR_EXIT_TIMEOUT ({PREDECESSOR_EXIT_TIMEOUT:?}; waited {elapsed:?})"
            ),
        )),
        (_, error) => Err(error),
    }
}
