//! Low-level PTY controller: spawn, inject, resize, drain, and cleanup.

use std::{
    io::{Read, Write},
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize as PortablePtySize};

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
    /// `env` pairs are applied after host-terminal hygiene stripping. The child
    /// inherits a cleaned environment rather than the raw host process env.
    pub fn spawn(
        binary: &Path,
        size: PtySize,
        args: &[impl AsRef<str>],
        env: &[(impl AsRef<str>, impl AsRef<str>)],
        cwd: Option<&Path>,
    ) -> Result<Self> {
        #[cfg(not(unix))]
        {
            let _ = (binary, size, args, env, cwd);
            anyhow::bail!("rho-tui-pty requires a Unix PTY; Windows is skipped for now");
        }

        #[cfg(unix)]
        {
            spawn_unix(binary, size, args, env, cwd)
        }
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
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }

    #[cfg(unix)]
    pub fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }
}

impl Drop for PtyController {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

#[cfg(unix)]
fn spawn_unix(
    binary: &Path,
    size: PtySize,
    args: &[impl AsRef<str>],
    env: &[(impl AsRef<str>, impl AsRef<str>)],
    cwd: Option<&Path>,
) -> Result<PtyController> {
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
    let reader_rx = spawn_reader(reader);

    Ok(PtyController {
        child,
        writer,
        reader_rx,
        master: pair.master,
        size,
        killed: false,
    })
}

fn apply_child_env(cmd: &mut CommandBuilder, env: &[(impl AsRef<str>, impl AsRef<str>)]) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    for color_var in ["NO_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
        cmd.env_remove(color_var);
    }
    for ssh_var in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
        cmd.env_remove(ssh_var);
    }
    for marker in crate::env::HOST_TERMINAL_MARKERS {
        cmd.env_remove(marker);
    }
    // Avoid accidental Herdr coupling during automated PTY runs.
    for herdr_var in ["HERDR_ENV", "HERDR_SOCKET_PATH", "HERDR_PANE_ID"] {
        cmd.env_remove(herdr_var);
    }
    for (key, value) in env {
        cmd.env(key.as_ref(), value.as_ref());
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("rho-tui-pty-reader".into())
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
        .expect("failed to spawn PTY reader thread");
    rx
}
