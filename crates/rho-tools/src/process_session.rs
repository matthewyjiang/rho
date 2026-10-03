//! Unix session isolation shared by shell tools and the application host.
//!
//! Internal cross-crate support for Rho, not a supported downstream API.

use std::os::unix::process::CommandExt;

/// Start the child as the leader of a new session with no controlling terminal.
///
/// Use for every child that must not interact with the host terminal. The new
/// session also creates a process group whose id is the child's pid, so
/// `kill(-pid, ..)` still reaches its descendants.
///
/// A child that shares the host's controlling terminal can take the terminal's
/// foreground group (an interactive shell does), which stops the TUI with
/// SIGTTIN on its next read. Without a controlling terminal, the child cannot do
/// that or change terminal modes, and opening `/dev/tty` fails fast with ENXIO
/// instead of stopping the child until its timeout.
pub fn start_new_session(command: &mut std::process::Command) {
    // SAFETY: the hook only calls `setsid`, which is async-signal-safe and does
    // not allocate, as required between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
