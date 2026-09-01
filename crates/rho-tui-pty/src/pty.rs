//! Low-level PTY controller: spawn, inject, resize, drain, and cleanup.

pub use rho_coding_agent::pty::PtySize;

#[cfg(unix)]
pub use rho_coding_agent::pty::PtyController;

#[cfg(not(unix))]
mod unsupported {
    use std::{path::Path, time::Duration};

    use anyhow::Result;

    use super::PtySize;

    /// Spawn and control a child process inside a pseudo-terminal.
    pub struct PtyController {
        _private: (),
    }

    impl PtyController {
        /// Spawn `binary` with `args` inside a PTY.
        ///
        /// `env` is the complete child environment after clearing the host process
        /// environment. Callers should pass every variable the child needs.
        pub fn spawn(
            binary: &Path,
            size: PtySize,
            args: &[impl AsRef<str>],
            env: &[(impl AsRef<str>, impl AsRef<str>)],
            cwd: Option<&Path>,
        ) -> Result<Self> {
            let _ = (binary, size, args, env, cwd);
            anyhow::bail!("rho-tui-pty requires a Unix PTY; Windows is skipped for now")
        }

        pub fn size(&self) -> PtySize {
            unreachable!("rho-tui-pty requires a Unix PTY")
        }

        pub fn inject_bytes(&mut self, bytes: &[u8]) -> Result<()> {
            let _ = bytes;
            unreachable!("rho-tui-pty requires a Unix PTY")
        }

        pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
            let _ = (rows, cols);
            unreachable!("rho-tui-pty requires a Unix PTY")
        }

        /// Receive one output chunk, waiting up to `timeout`.
        pub fn recv_chunk(&self, timeout: Duration) -> Option<Vec<u8>> {
            let _ = timeout;
            unreachable!("rho-tui-pty requires a Unix PTY")
        }

        /// Drain all currently available output for up to `timeout`.
        pub fn drain(&self, timeout: Duration) -> Vec<u8> {
            let _ = timeout;
            unreachable!("rho-tui-pty requires a Unix PTY")
        }

        pub fn is_running(&mut self) -> bool {
            unreachable!("rho-tui-pty requires a Unix PTY")
        }

        /// Wait for the child to exit and return its exit code.
        pub fn wait_exit(&mut self, timeout: Duration) -> Result<Option<u32>> {
            let _ = timeout;
            unreachable!("rho-tui-pty requires a Unix PTY")
        }

        pub fn kill(&mut self) -> Result<()> {
            unreachable!("rho-tui-pty requires a Unix PTY")
        }
    }
}

#[cfg(not(unix))]
pub use unsupported::PtyController;
