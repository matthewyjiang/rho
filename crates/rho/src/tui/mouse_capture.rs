#[cfg(windows)]
use std::io::Write;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
};

const ENABLE_VT_MOUSE_CAPTURE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
const DISABLE_VT_MOUSE_CAPTURE: &[u8] = b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

// Console mode bits from
// https://learn.microsoft.com/en-us/windows/console/setconsolemode
const ENABLE_MOUSE_INPUT: u32 = 0x0010;
const ENABLE_WINDOW_INPUT: u32 = 0x0008;
const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;

pub(super) fn enable() -> std::io::Result<()> {
    execute!(std::io::stdout(), EnableMouseCapture)?;
    // Crossterm's Windows path only sets console mouse flags and intentionally
    // skips ANSI mouse-tracking sequences. Terminals such as Windows Terminal
    // and WezTerm need the VT sequences to deliver wheel events to the app
    // instead of converting them to arrow keys in the alternate screen.
    let _ = configure_windows_console_mouse_input();
    let _ = write_windows_vt_mouse_capture(ENABLE_VT_MOUSE_CAPTURE);
    Ok(())
}

pub(super) fn disable() -> std::io::Result<()> {
    let _ = write_windows_vt_mouse_capture(DISABLE_VT_MOUSE_CAPTURE);
    execute!(std::io::stdout(), DisableMouseCapture)
}

/// Re-assert mouse tracking after focus changes. Some Windows terminal hosts
/// drop application mouse mode when the window is refocused.
pub(super) fn reassert() {
    let _ = configure_windows_console_mouse_input();
    let _ = write_windows_vt_mouse_capture(ENABLE_VT_MOUSE_CAPTURE);
}

/// Console mode required for application mouse reporting on Windows.
///
/// Quick Edit must be cleared whenever `ENABLE_EXTENDED_FLAGS` is set;
/// otherwise the console steals mouse input for text selection and wheel
/// events never reach the application (notably under WezTerm).
///
/// Kept available on non-Windows so unit tests can lock the bit math without a
/// Windows target.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn windows_mouse_input_mode(current: u32) -> u32 {
    (current | ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT | ENABLE_EXTENDED_FLAGS)
        & !ENABLE_QUICK_EDIT_MODE
}

#[cfg(windows)]
fn configure_windows_console_mouse_input() -> std::io::Result<()> {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, STD_INPUT_HANDLE,
    };

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle == -1isize as _ {
        return Err(std::io::Error::last_os_error());
    }

    let mut current = 0;
    if unsafe { GetConsoleMode(handle, &mut current) } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let desired = windows_mouse_input_mode(current);
    if desired == current {
        return Ok(());
    }
    if unsafe { SetConsoleMode(handle, desired) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn configure_windows_console_mouse_input() -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn write_windows_vt_mouse_capture(sequence: &[u8]) -> std::io::Result<()> {
    // Always emit the sequences on Windows. `supports_ansi()` can be false in
    // some ConPTY hosts even when the outer terminal understands mouse modes
    // (WezTerm is one such case). Writing is harmless if ignored.
    let mut stdout = std::io::stdout();
    stdout.write_all(sequence)?;
    stdout.flush()
}

#[cfg(not(windows))]
fn write_windows_vt_mouse_capture(_sequence: &[u8]) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "mouse_capture_tests.rs"]
mod tests;
