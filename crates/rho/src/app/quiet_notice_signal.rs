//! Debug builds under the fixture matrix tell the quiet-notice PTY scenarios
//! each time a parent looked at the notices its children queued and started
//! no turn for them. A scenario releases its next notice only then, so it
//! proves the earlier one bought no provider request, not merely that the
//! child queued it.
//!
//! The scenario listens on a datagram socket in its workspace. The daemon,
//! whose working directory is `/`, finds the workspace as its fixtures do.

use std::path::{Path, PathBuf};

/// The scenario's socket, in its workspace.
const SOCKET: &str = ".quiet-parent-pty.sock";

/// Names the workspace the PTY harness reads and writes fixture files in
/// (see `rho-providers`' `tui_fixture/release.rs`).
const WORKSPACE_ENV: &str = "RHO_TUI_FIXTURE_WORKSPACE";

fn matrix_enabled() -> bool {
    std::env::var_os("RHO_TUI_TEST_MODE").as_deref() == Some(std::ffi::OsStr::new("matrix"))
}

/// Where the scenario listens.
pub(crate) fn socket() -> PathBuf {
    match std::env::var_os(WORKSPACE_ENV) {
        Some(workspace) => PathBuf::from(workspace).join(SOCKET),
        None => PathBuf::from(SOCKET),
    }
}

/// Tells the scenario listening at `socket` that the parent looked with
/// `queued` notices waiting. Never blocks: a full or retired listener fails
/// the send.
#[cfg(unix)]
pub(crate) fn send(queued: usize, socket: &Path) -> std::io::Result<()> {
    let sender = std::os::unix::net::UnixDatagram::unbound()?;
    sender.set_nonblocking(true)?;
    sender.send_to(format!("boundary:{queued}").as_bytes(), socket)?;
    Ok(())
}

/// The scenarios that listen run on Unix only, so elsewhere there is nobody
/// to tell.
#[cfg(not(unix))]
pub(crate) fn send(_queued: usize, _socket: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Tells the scenario, under the fixture matrix, that a parent looked with
/// `queued` notices waiting and started no turn. Nothing to tell without a
/// notice. A scenario that is not listening is not an error.
pub(crate) fn parent_looked(queued: usize) {
    if queued > 0 && matrix_enabled() {
        let _ = send(queued, &socket());
    }
}
