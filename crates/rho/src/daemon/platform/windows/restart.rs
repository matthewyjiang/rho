//! Restarting (#1778): Windows cannot replace a running process with
//! another program, and a file lock does not pass to a child, so the daemon
//! stops as for any stop and starts its binary as a new process.
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
};

use super::{endpoint::Listener, process::detach};
use crate::daemon::{paths::DaemonPaths, server::Restarting};

/// Stops this daemon, then starts `executable` with the arguments `args`
/// makes without a handoff token: the new process takes the lock and
/// creates the pipe itself. `Ok` once it started, and this process should
/// exit. An error comes once the lock is released and the pipe closed, so
/// the next client starts a new daemon.
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
        .args(args(None))
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

/// A restart here hands nothing over, so no daemon is run with a token.
pub(in crate::daemon) fn take_handoff(
    _paths: &DaemonPaths,
    token: &str,
) -> io::Result<(File, Listener)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("a rho daemon on Windows restarts as a new process and takes no handoff, but got {token:?}"),
    ))
}
