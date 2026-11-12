//! The Windows daemon as the harness reaches it: the named pipe its record
//! names, and the stop pipe beside it.

use std::{ffi::OsString, fs, os::windows::fs::OpenOptionsExt, path::PathBuf, process::Command};

/// `SECURITY_IDENTIFICATION`, from the Windows SDK.
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

use super::{fail_teardown, DAEMON_TEARDOWN_TIMEOUT};
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

/// Waits for `pid` to exit. Nothing is left to reap on Windows, but the
/// daemon releases its lock when its run returns, a moment before the
/// process is gone, and until then it holds what it inherited from the test
/// that started it, the test runner's output pipes among them.
///
/// A daemon still running [`DAEMON_TEARDOWN_TIMEOUT`] later is killed.
pub(super) fn reap(pid: u32) {
    // SAFETY: the call takes no pointers.
    let process = unsafe {
        OpenProcess(PROCESS_SYNCHRONIZE, /*bInheritHandle*/ 0, pid)
    };
    if process.is_null() {
        // No process has that pid any more, or it is not this user's.
        return;
    }
    // SAFETY: `process` is the open handle `OpenProcess` returned, closed
    // once, after the wait.
    let exited = unsafe {
        let waited = WaitForSingleObject(process, DAEMON_TEARDOWN_TIMEOUT.as_millis() as u32);
        CloseHandle(process);
        waited == WAIT_OBJECT_0
    };
    if !exited {
        kill(pid);
        fail_teardown(&format!(
            "rho daemon (pid {pid}) released its lock but had not exited DAEMON_TEARDOWN_TIMEOUT ({DAEMON_TEARDOWN_TIMEOUT:?}) later"
        ));
    }
}
