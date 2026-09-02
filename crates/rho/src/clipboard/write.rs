use std::io;

use crossterm::{clipboard::CopyToClipboard, execute};

use super::{
    process::{command_available, command_output, write_command_stdin},
    session::SessionKind,
};

/// Describes how strongly a clipboard write was verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyOutcome {
    /// A host clipboard backend accepted the contents.
    Confirmed,
    /// The request reached the terminal, but OSC 52 is not acknowledged.
    SentToTerminal,
}

/// Reads and writes host clipboard text through the best backend for this session.
pub struct SystemClipboard {
    session: SessionKind,
    native: Option<arboard::Clipboard>,
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemClipboard {
    pub fn new() -> Self {
        Self {
            session: SessionKind::detect(),
            native: None,
        }
    }

    pub fn copy_text(&mut self, text: &str) -> io::Result<CopyOutcome> {
        match self.session {
            SessionKind::Remote => copy_to_terminal(text).map(|()| CopyOutcome::SentToTerminal),
            SessionKind::Local => match self.copy_native(text) {
                Ok(()) => Ok(CopyOutcome::Confirmed),
                Err(native_error) => {
                    fallback_to_terminal(copy_to_terminal(text), Some(native_error))
                }
            },
            SessionKind::Wsl => self.copy_wsl(text),
        }
    }

    /// Reads clipboard text for the current session.
    ///
    /// An empty clipboard is `Ok("")`. Remote sessions cannot read a host
    /// clipboard (OSC 52 is write-only).
    pub fn paste_text(&mut self) -> io::Result<String> {
        paste_text_with(
            self.session,
            || self.paste_native(),
            paste_from_windows_host_clipboard,
        )
    }

    fn copy_wsl(&mut self, text: &str) -> io::Result<CopyOutcome> {
        // Prefer the Windows host clipboard. WSLg native access is a secondary path.
        match copy_to_windows_host_clipboard(text) {
            Ok(()) => Ok(CopyOutcome::Confirmed),
            Err(windows_error) => match self.copy_native(text) {
                Ok(()) => Ok(CopyOutcome::Confirmed),
                Err(native_error) => fallback_to_terminal(
                    copy_to_terminal(text),
                    Some(join_host_errors(windows_error, native_error)),
                ),
            },
        }
    }

    fn copy_native(&mut self, text: &str) -> io::Result<()> {
        self.with_native(|clipboard| clipboard.set_text(text).map_err(io_error_from_native))
    }

    fn paste_native(&mut self) -> io::Result<String> {
        self.with_native(|clipboard| clipboard_text_from_native(clipboard.get_text()))
    }

    fn with_native<T>(
        &mut self,
        op: impl FnOnce(&mut arboard::Clipboard) -> io::Result<T>,
    ) -> io::Result<T> {
        if let Some(clipboard) = self.native.as_mut() {
            return match op(clipboard) {
                Ok(value) => Ok(value),
                Err(error) => {
                    self.native = None;
                    Err(error)
                }
            };
        }

        let mut clipboard = arboard::Clipboard::new().map_err(io_error_from_native)?;
        let value = op(&mut clipboard)?;
        self.native = Some(clipboard);
        Ok(value)
    }
}

fn paste_text_with(
    session: SessionKind,
    mut paste_native: impl FnMut() -> io::Result<String>,
    paste_windows: impl Fn() -> io::Result<String>,
) -> io::Result<String> {
    match session {
        SessionKind::Remote => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Remote session detected. Text paste needs a local host clipboard and is unavailable over SSH/Mosh.",
        )),
        SessionKind::Local => paste_native(),
        SessionKind::Wsl => match paste_windows() {
            Ok(text) => Ok(text),
            Err(windows_error) => match paste_native() {
                Ok(text) => Ok(text),
                Err(native_error) => Err(join_host_errors(windows_error, native_error)),
            },
        },
    }
}

fn clipboard_text_from_native(result: Result<String, arboard::Error>) -> io::Result<String> {
    match result {
        Ok(text) => Ok(text),
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(error) => Err(io_error_from_native(error)),
    }
}

fn paste_from_windows_host_clipboard() -> io::Result<String> {
    // Write instead of Out-Default so PowerShell does not append a pipeline newline.
    let output = command_output(
        "powershell.exe",
        &[
            "-NoProfile",
            "-Command",
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $t = Get-Clipboard -Raw; if ($null -ne $t) { [Console]::Out.Write($t) }",
        ],
    )
    .ok_or_else(|| io::Error::other("powershell.exe Get-Clipboard failed"))?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

pub(super) struct TextWriteProbe {
    pub status: &'static str,
    pub healthy: bool,
    pub detail: String,
}

pub(super) fn probe_text_write(session: SessionKind) -> TextWriteProbe {
    probe_text_write_with(session, command_available, native_clipboard_available)
}

pub(super) fn probe_text_write_with(
    session: SessionKind,
    host_command_available: impl Fn(&str) -> bool,
    native_available: impl Fn() -> bool,
) -> TextWriteProbe {
    match session {
        SessionKind::Remote => TextWriteProbe {
            status: "osc 52",
            // Remote has no host clipboard; OSC 52 is the intended path.
            healthy: true,
            detail: "Remote session detected. Text copy uses OSC 52 so the client terminal owns the clipboard.".into(),
        },
        SessionKind::Wsl => {
            if host_command_available("clip.exe") {
                TextWriteProbe {
                    status: "windows host",
                    healthy: true,
                    detail: "WSL session. Text copy uses clip.exe on the Windows host, then native API, then OSC 52.".into(),
                }
            } else if native_available() {
                TextWriteProbe {
                    status: "native",
                    healthy: true,
                    detail: "WSL session without clip.exe. Text copy uses the native clipboard API, then OSC 52.".into(),
                }
            } else {
                TextWriteProbe {
                    status: "osc 52 fallback",
                    healthy: false,
                    detail: "WSL session without clip.exe or a native clipboard. Text copy falls back to unconfirmed OSC 52.".into(),
                }
            }
        }
        SessionKind::Local => {
            if native_available() {
                TextWriteProbe {
                    status: "native",
                    healthy: true,
                    detail: "Local session. Text copy uses the native clipboard API, with OSC 52 fallback.".into(),
                }
            } else {
                TextWriteProbe {
                    status: "osc 52 fallback",
                    healthy: false,
                    detail: "Native clipboard unavailable. Text copy falls back to unconfirmed OSC 52.".into(),
                }
            }
        }
    }
}

fn copy_to_terminal(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, CopyToClipboard::to_clipboard_from(text))
}

fn copy_to_windows_host_clipboard(text: &str) -> io::Result<()> {
    // clip.exe reads UTF-16LE. A BOM keeps Windows from treating the payload as ANSI.
    write_command_stdin("clip.exe", &[], &utf16_le_bom_bytes(text))
}

fn utf16_le_bom_bytes(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 + text.len().saturating_mul(2));
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn fallback_to_terminal(
    terminal_result: io::Result<()>,
    host_error: Option<io::Error>,
) -> io::Result<CopyOutcome> {
    match terminal_result {
        Ok(()) => Ok(CopyOutcome::SentToTerminal),
        Err(terminal_error) => Err(match host_error {
            None => terminal_error,
            Some(host_error) => io::Error::new(
                terminal_error.kind(),
                format!("{terminal_error} (host clipboard: {host_error})"),
            ),
        }),
    }
}

fn join_host_errors(first: io::Error, second: io::Error) -> io::Error {
    io::Error::other(format!("{first}; {second}"))
}

fn native_clipboard_available() -> bool {
    arboard::Clipboard::new().is_ok()
}

fn io_error_from_native(error: arboard::Error) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
