//! A FIFO a fixture command blocks on until the scenario releases it: the
//! command's read waits in the real process, with no polling shell and no
//! timing-based release.

use std::{
    ffi::CString,
    fs::OpenOptions,
    io::{ErrorKind, Write},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};

use super::STREAM;
use crate::PtyHarness;

/// Creates the FIFO at `path`. A command that reads it blocks until
/// [`release`] writes to it.
pub(super) fn create(path: &Path) -> Result<()> {
    let fifo = CString::new(path.as_os_str().as_bytes())?;
    let created = unsafe { libc::mkfifo(fifo.as_ptr(), libc::S_IRUSR | libc::S_IWUSR) };
    if created != 0 {
        return Err(std::io::Error::last_os_error()).context("create release FIFO");
    }
    Ok(())
}

/// Writes `release` to the FIFO at `path` once a command reads it, which
/// lets one reader go on.
pub(super) fn release(harness: &mut PtyHarness, path: &Path) -> Result<()> {
    let deadline = Instant::now() + STREAM.duration;
    loop {
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(mut writer) => {
                return writer
                    .write_all(b"release\n")
                    .context("release the command reading the FIFO")
            }
            Err(error)
                if error.raw_os_error() == Some(libc::ENXIO)
                    || error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(error).context("open release FIFO"),
        }
        ensure!(
            harness.is_running() && Instant::now() < deadline,
            "no command opened its release FIFO {}:\n{}",
            path.display(),
            harness.screen().debug_dump()
        );
        // Poll PTY output while waiting on the explicit reader-ready condition.
        harness.pump(Duration::from_millis(25));
    }
}
