//! VT100-backed Unix PTY session for the Claude `/usage` probe.

use std::{path::Path, time::Duration};

use vt100::Parser;

use crate::pty::{PtyController, PtySize};

/// Killed on drop via the inner controller.
pub(super) struct PtySession {
    inner: PtyController,
    parser: Parser,
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
        let inner = PtyController::spawn(binary, PtySize::new(rows, cols), args, env, Some(cwd))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            inner,
            parser: Parser::new(rows, cols, 0),
        })
    }

    pub(super) fn inject_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.inner
            .inject_bytes(bytes)
            .map_err(|error| error.to_string())
    }

    pub(super) fn poll(&mut self, budget: Duration) {
        let chunk = self.inner.drain(budget);
        if !chunk.is_empty() {
            self.parser.process(&chunk);
        }
    }

    pub(super) fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    pub(super) fn is_running(&mut self) -> bool {
        self.inner.is_running()
    }
}
