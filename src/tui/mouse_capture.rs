#[cfg(windows)]
use std::io::Write;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
};

const ENABLE_VT_MOUSE_CAPTURE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
const DISABLE_VT_MOUSE_CAPTURE: &[u8] = b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

pub(super) fn enable() -> std::io::Result<()> {
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let _ = configure_windows_vt_mouse_capture(ENABLE_VT_MOUSE_CAPTURE);
    Ok(())
}

pub(super) fn disable() -> std::io::Result<()> {
    let _ = configure_windows_vt_mouse_capture(DISABLE_VT_MOUSE_CAPTURE);
    execute!(std::io::stdout(), DisableMouseCapture)
}

#[cfg(windows)]
fn configure_windows_vt_mouse_capture(sequence: &[u8]) -> std::io::Result<()> {
    if !crossterm::ansi_support::supports_ansi() {
        return Ok(());
    }

    let mut stdout = std::io::stdout();
    stdout.write_all(sequence)?;
    stdout.flush()
}

#[cfg(not(windows))]
fn configure_windows_vt_mouse_capture(_sequence: &[u8]) -> std::io::Result<()> {
    Ok(())
}
