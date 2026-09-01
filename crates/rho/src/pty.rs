//! Low-level PTY session: spawn, inject, resize, drain, and cleanup.
//!
//! Shared by the Claude `/usage` probe and the unpublished TUI PTY harness.

/// Terminal size in character cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

impl PtySize {
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

#[cfg(unix)]
mod unix {
    use std::{
        io::{Read, Write},
        path::Path,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use anyhow::{Context, Result};
    use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize as PortablePtySize};

    use super::PtySize;

    impl From<PtySize> for PortablePtySize {
        fn from(value: PtySize) -> Self {
            Self {
                rows: value.rows,
                cols: value.cols,
                pixel_width: 0,
                pixel_height: 0,
            }
        }
    }

    /// Spawn and control a child process inside a pseudo-terminal.
    pub struct PtyController {
        child: Box<dyn portable_pty::Child + Send + Sync>,
        writer: Box<dyn Write + Send>,
        reader_rx: mpsc::Receiver<Vec<u8>>,
        master: Box<dyn MasterPty + Send>,
        size: PtySize,
        killed: bool,
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
            let pty_system = native_pty_system();
            let pair = pty_system
                .openpty(size.into())
                .context("failed to open PTY")?;

            let mut cmd = CommandBuilder::new(binary);
            for arg in args {
                cmd.arg(arg.as_ref());
            }
            if let Some(dir) = cwd {
                cmd.cwd(dir);
            }
            apply_child_env(&mut cmd, env);

            let child = pair
                .slave
                .spawn_command(cmd)
                .with_context(|| format!("failed to spawn {}", binary.display()))?;
            drop(pair.slave);

            let reader = pair
                .master
                .try_clone_reader()
                .context("failed to clone PTY reader")?;
            let writer = pair
                .master
                .take_writer()
                .context("failed to take PTY writer")?;
            let reader_rx = spawn_reader(reader)?;

            Ok(Self {
                child,
                writer,
                reader_rx,
                master: pair.master,
                size,
                killed: false,
            })
        }

        pub fn size(&self) -> PtySize {
            self.size
        }

        pub fn inject_bytes(&mut self, bytes: &[u8]) -> Result<()> {
            self.writer
                .write_all(bytes)
                .context("failed to write to PTY stdin")?;
            self.writer.flush().ok();
            Ok(())
        }

        pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
            self.master
                .resize(PortablePtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .context("failed to resize PTY")?;
            self.size = PtySize::new(rows, cols);
            Ok(())
        }

        /// Receive one output chunk, waiting up to `timeout`.
        pub fn recv_chunk(&self, timeout: Duration) -> Option<Vec<u8>> {
            self.reader_rx.recv_timeout(timeout).ok()
        }

        /// Drain all currently available output for up to `timeout`.
        pub fn drain(&self, timeout: Duration) -> Vec<u8> {
            let mut out = Vec::new();
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match self.reader_rx.recv_timeout(remaining) {
                    Ok(chunk) => out.extend(chunk),
                    Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                        break
                    }
                }
            }
            out
        }

        pub fn is_running(&mut self) -> bool {
            matches!(self.child.try_wait(), Ok(None))
        }

        /// Wait for the child to exit and return its exit code.
        pub fn wait_exit(&mut self, timeout: Duration) -> Result<Option<u32>> {
            let deadline = Instant::now() + timeout;
            loop {
                match self.child.try_wait() {
                    Ok(Some(status)) => return Ok(Some(status.exit_code())),
                    Ok(None) if Instant::now() >= deadline => return Ok(None),
                    Ok(None) => thread::sleep(Duration::from_millis(20)),
                    Err(error) => return Err(error).context("failed to wait for PTY child"),
                }
            }
        }

        pub fn kill(&mut self) -> Result<()> {
            if self.killed {
                return Ok(());
            }
            self.killed = true;
            // portable-pty `setsid`s the child, so the pid is the group leader.
            if let Some(pid) = self.child.process_id() {
                if let Ok(pid) = i32::try_from(pid) {
                    // SAFETY: pid is the session leader portable-pty created.
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
            Ok(())
        }

        pub fn child_pid(&self) -> Option<u32> {
            self.child.process_id()
        }
    }

    impl Drop for PtyController {
        fn drop(&mut self) {
            let _ = self.kill();
        }
    }

    fn apply_child_env(cmd: &mut CommandBuilder, env: &[(impl AsRef<str>, impl AsRef<str>)]) {
        // Isolated runs get only the launch-plan environment. Clearing first keeps
        // host terminal markers and provider credentials out without maintaining a
        // separate deny list that drifts as new secrets are added.
        cmd.env_clear();
        for (key, value) in env {
            cmd.env(key.as_ref(), value.as_ref());
        }
    }

    fn spawn_reader(mut reader: Box<dyn Read + Send>) -> Result<mpsc::Receiver<Vec<u8>>> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("rho-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("failed to spawn PTY reader thread")?;
        Ok(rx)
    }
}

#[cfg(unix)]
pub use unix::PtyController;
