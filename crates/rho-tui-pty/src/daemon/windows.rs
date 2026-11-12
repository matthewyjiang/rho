//! The Windows daemon as the harness reaches it: the named pipe its record
//! names, and the stop pipe beside it.

use std::{ffi::OsString, fs, os::windows::fs::OpenOptionsExt, path::PathBuf, process::Command};

/// `SECURITY_IDENTIFICATION`, from the Windows SDK.
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

use crate::env::IsolatedHome;

/// The pipe the recorded daemon listens on, while it exists. Pipes are
/// listed rather than opened: opening one is a connection.
pub(super) fn socket(home: &IsolatedHome) -> Option<PathBuf> {
    let socket = recorded_socket(home)?;
    let name = socket.file_name()?.to_owned();
    fs::read_dir(r"\\.\pipe\")
        .ok()?
        .flatten()
        .any(|pipe| pipe.file_name() == name)
        .then_some(socket)
}

/// The pipe the daemon's record names.
fn recorded_socket(home: &IsolatedHome) -> Option<PathBuf> {
    Some(home.daemon_record()?.get("socket")?.as_str()?.into())
}

/// Connects to the recorded daemon's stop pipe, `<its pipe>-stop`, which is
/// how a daemon is asked to stop outside the protocol. Refused when the
/// record is stale: its daemon's pipes went with it.
pub(super) fn ask_to_stop(home: &IsolatedHome, _pid: u32) -> bool {
    let Some(socket) = recorded_socket(home) else {
        return false;
    };
    let mut stop = OsString::from(socket);
    stop.push("-stop");
    fs::File::options()
        .read(true)
        .write(true)
        // The pipe's server may only learn who connected, as rho's own
        // clients allow it.
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(stop)
        .is_ok()
}

/// Ends `pid` and the processes it started.
pub(super) fn kill(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
}

/// Nothing to reap: a process that exited is gone once its handles close.
pub(super) fn reap(_pid: u32) {}
