//! Minimal Unix PTY session for the Claude `/usage` probe.
//!
//! Spawn, inject, drain into a VT screen, kill. Not a test harness.

use std::{
    io::{Read, Write},
    path::Path,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty};
use vt100::Parser;

pub(super) struct PtySession {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    reader_rx: mpsc::Receiver<Vec<u8>>,
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    parser: Parser,
    killed: bool,
}

impl PtySession {
    pub(super) fn spawn(
        binary: &Path,
        args: &[&str],
        env: &[(String, String)],
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<Self, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;

        let mut cmd = CommandBuilder::new(binary);
        for arg in args {
            cmd.arg(*arg);
        }
        cmd.cwd(cwd);
        cmd.env_clear();
        for (key, value) in env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|error| format!("failed to spawn {}: {error}", binary.display()))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        let (tx, reader_rx) = mpsc::channel();
        thread::Builder::new()
            .name("rho-claude-usage-pty".into())
            .spawn(move || {
                let mut reader = reader;
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
            .map_err(|error| error.to_string())?;

        Ok(Self {
            child,
            writer,
            reader_rx,
            master: pair.master,
            parser: Parser::new(rows, cols, 0),
            killed: false,
        })
    }

    pub(super) fn inject_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.writer
            .write_all(bytes)
            .map_err(|error| error.to_string())?;
        let _ = self.writer.flush();
        Ok(())
    }

    pub(super) fn poll(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.reader_rx.recv_timeout(remaining) {
                Ok(chunk) => self.parser.process(&chunk),
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                    break
                }
            }
        }
    }

    pub(super) fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    pub(super) fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub(super) fn kill(&mut self) {
        if self.killed {
            return;
        }
        self.killed = true;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.kill();
    }
}
