use std::{
    io::{self, Write},
    process::{Command, Stdio},
};

use crossterm::{clipboard::CopyToClipboard, execute};

use super::session::SessionKind;

/// Describes how strongly a clipboard write was verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyOutcome {
    /// A host clipboard backend accepted the contents.
    Confirmed,
    /// The request reached the terminal, but OSC 52 is not acknowledged.
    SentToTerminal,
}

/// Writes transcript text through the best host clipboard for this session.
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
        if let Some(clipboard) = self.native.as_mut() {
            return match clipboard.set_text(text) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.native = None;
                    Err(io::Error::other(error.to_string()))
                }
            };
        }

        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| io::Error::other(error.to_string()))?;
        clipboard
            .set_text(text)
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.native = Some(clipboard);
        Ok(())
    }
}

pub(super) struct TextWriteProbe {
    pub status: &'static str,
    pub healthy: bool,
    pub detail: String,
}

pub(super) fn probe_text_write(session: SessionKind) -> TextWriteProbe {
    match session {
        SessionKind::Remote => TextWriteProbe {
            status: "osc 52",
            healthy: true,
            detail: "Remote session detected. Text copy uses OSC 52 so the client terminal owns the clipboard.".into(),
        },
        SessionKind::Wsl => {
            if command_available("clip.exe") {
                TextWriteProbe {
                    status: "windows host",
                    healthy: true,
                    detail: "WSL session. Text copy uses clip.exe on the Windows host, then native API, then OSC 52.".into(),
                }
            } else if native_clipboard_available() {
                TextWriteProbe {
                    status: "native",
                    healthy: true,
                    detail: "WSL session without clip.exe. Text copy uses the native clipboard API, then OSC 52.".into(),
                }
            } else {
                TextWriteProbe {
                    status: "osc 52 fallback",
                    healthy: true,
                    detail: "WSL session without clip.exe or a native clipboard. Text copy falls back to OSC 52.".into(),
                }
            }
        }
        SessionKind::Local => {
            if native_clipboard_available() {
                TextWriteProbe {
                    status: "native",
                    healthy: true,
                    detail: "Local session. Text copy uses the native clipboard API, with OSC 52 fallback.".into(),
                }
            } else {
                TextWriteProbe {
                    status: "osc 52 fallback",
                    healthy: true,
                    detail: "Native clipboard unavailable. Text copy falls back to OSC 52.".into(),
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

fn write_command_stdin(program: &str, args: &[&str], bytes: &[u8]) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| io::Error::new(error.kind(), format!("spawn {program}: {error}")))?;

    let write_result = (|| {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, format!("{program} stdin closed"))
        })?;
        stdin.write_all(bytes)?;
        stdin.flush()?;
        Ok(())
    })();

    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} exited with {status}")))
    }
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

pub(super) fn command_available(command: &str) -> bool {
    Command::new(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .arg("--help")
        .status()
        .map(|status| status.success() || status.code().is_some())
        .unwrap_or_else(|_| {
            // Some Windows tools reject --help but still spawn.
            Command::new(command)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .is_ok()
        })
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
