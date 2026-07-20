use std::io;

use crossterm::{clipboard::CopyToClipboard, execute};

/// Describes how strongly a clipboard write was verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyOutcome {
    /// The operating system accepted the clipboard contents.
    Confirmed,
    /// The request reached the terminal, but the terminal does not acknowledge OSC 52 writes.
    SentToTerminal,
}

/// Writes transcript text to the user's clipboard synchronously.
///
/// Implementors must preserve the supplied text and report whether the destination confirmed the
/// write. Errors mean that no available backend accepted the request.
pub(super) trait ClipboardWriter {
    fn copy(&mut self, text: &str) -> io::Result<CopyOutcome>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionKind {
    Local,
    Remote,
}

impl SessionKind {
    fn detect() -> Self {
        let remote_markers = ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "MOSH_IP"];
        if remote_markers
            .iter()
            .any(|name| std::env::var_os(name).is_some())
        {
            Self::Remote
        } else {
            Self::Local
        }
    }
}

/// Uses the host clipboard locally and the terminal clipboard across SSH.
pub(super) struct SystemClipboard {
    session: SessionKind,
    native: Option<arboard::Clipboard>,
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self {
            session: SessionKind::detect(),
            native: None,
        }
    }
}

impl ClipboardWriter for SystemClipboard {
    fn copy(&mut self, text: &str) -> io::Result<CopyOutcome> {
        let session = self.session;
        copy_with_backends(
            session,
            text,
            |text| self.copy_to_native_clipboard(text),
            copy_to_terminal,
        )
    }
}

impl SystemClipboard {
    fn copy_to_native_clipboard(&mut self, text: &str) -> Result<(), arboard::Error> {
        if self.native.is_none() {
            self.native = Some(arboard::Clipboard::new()?);
        }
        self.native
            .as_mut()
            .expect("native clipboard was initialized")
            .set_text(text)
    }
}

fn copy_with_backends<NativeError>(
    session: SessionKind,
    text: &str,
    copy_native: impl FnOnce(&str) -> Result<(), NativeError>,
    copy_terminal: impl FnOnce(&str) -> io::Result<()>,
) -> io::Result<CopyOutcome> {
    match session {
        SessionKind::Local if copy_native(text).is_ok() => Ok(CopyOutcome::Confirmed),
        SessionKind::Local | SessionKind::Remote => {
            copy_terminal(text).map(|()| CopyOutcome::SentToTerminal)
        }
    }
}

fn copy_to_terminal(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, CopyToClipboard::to_clipboard_from(text))
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
