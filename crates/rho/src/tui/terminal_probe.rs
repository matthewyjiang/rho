//! Shared startup terminal queries and response collection. Runs before the
//! event reader takes stdin; DA1 terminates replies for all queried capabilities.

use super::program_status::{self, ProgramStatusSupport};
#[cfg(windows)]
use super::theme_terminal::query_windows_console_palette;
use super::theme_terminal::{
    matrix_fixture_palette, parse_palette_response, write_palette_queries, TerminalPalette,
};

/// Primary device attributes: its reply ends earlier capability replies.
const DEVICE_ATTRIBUTES_QUERY: &[u8] = b"\x1b[c";

/// What the startup probe learned about the host terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalProbe {
    pub palette: Option<TerminalPalette>,
    pub program_status: ProgramStatusSupport,
}

impl TerminalProbe {
    const UNANSWERED: Self = Self {
        palette: None,
        program_status: ProgramStatusSupport::Unsupported,
    };
}

/// Queries the terminal before any event reader owns stdin. Waits for the
/// device attributes reply, with the existing 80 ms deadline as a backstop.
pub(super) fn probe_terminal() -> TerminalProbe {
    if std::env::var_os("RHO_TUI_MATRIX_PALETTE").is_some_and(|value| value == "github-dark") {
        return TerminalProbe {
            palette: Some(matrix_fixture_palette()),
            program_status: ProgramStatusSupport::Unsupported,
        };
    }
    probe_terminal_impl().unwrap_or(TerminalProbe::UNANSWERED)
}

fn write_probe_queries(output: &mut impl std::io::Write) -> std::io::Result<()> {
    // Support replies must precede the final device attributes sentinel.
    output.write_all(program_status::QUERY)?;
    write_palette_queries(output)?;
    output.write_all(DEVICE_ATTRIBUTES_QUERY)?;
    output.flush()
}

#[cfg(unix)]
fn probe_terminal_impl() -> std::io::Result<TerminalProbe> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    let mut stdout = std::io::stdout();
    write_probe_queries(&mut stdout)?;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Ok(TerminalProbe::UNANSWERED);
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Ok(TerminalProbe::UNANSWERED);
    }

    let mut bytes = Vec::new();
    let mut probe = TerminalProbe::UNANSWERED;
    let deadline = Instant::now() + Duration::from_millis(80);
    let mut handle = stdin.lock();
    let mut complete = false;
    while Instant::now() < deadline && !complete {
        let mut buffer = [0u8; 1024];
        match handle.read(&mut buffer) {
            Ok(0) => std::thread::sleep(Duration::from_millis(2)),
            Ok(count) => {
                bytes.extend_from_slice(&buffer[..count]);
                let response = String::from_utf8_lossy(&bytes);
                probe = parse_probe_response(&response);
                complete = has_device_attributes_reply(&response);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }

    let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    Ok(probe)
}

#[cfg(windows)]
fn is_native_wezterm() -> bool {
    std::env::var_os("WEZTERM_PANE").is_some()
}

#[cfg(windows)]
fn probe_terminal_impl() -> std::io::Result<TerminalProbe> {
    if is_native_wezterm() {
        // WezTerm's bundled ConPTY does not pass terminal query responses back
        // to native Windows applications. Use the console palette directly.
        return Ok(TerminalProbe {
            palette: query_windows_console_palette().ok().flatten(),
            program_status: ProgramStatusSupport::Unsupported,
        });
    }

    use std::io::stdout;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, PeekConsoleInputW, ReadConsoleInputW, SetConsoleMode,
        ENABLE_VIRTUAL_TERMINAL_INPUT, INPUT_RECORD, KEY_EVENT, STD_INPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    struct ConsoleModeGuard {
        handle: *mut std::ffi::c_void,
        mode: u32,
    }

    impl Drop for ConsoleModeGuard {
        fn drop(&mut self) {
            unsafe { SetConsoleMode(self.handle, self.mode) };
        }
    }

    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if input.is_null() || input == -1isize as _ {
        return Ok(TerminalProbe::UNANSWERED);
    }

    let mut original_mode = 0;
    if unsafe { GetConsoleMode(input, &mut original_mode) } == 0 {
        return Ok(TerminalProbe::UNANSWERED);
    }
    if unsafe { SetConsoleMode(input, original_mode | ENABLE_VIRTUAL_TERMINAL_INPUT) } == 0 {
        return Ok(TerminalProbe::UNANSWERED);
    }
    let _mode_guard = ConsoleModeGuard {
        handle: input,
        mode: original_mode,
    };

    let mut output = stdout();
    write_probe_queries(&mut output)?;

    let mut bytes = Vec::new();
    let mut probe = TerminalProbe::UNANSWERED;
    let deadline = Instant::now() + Duration::from_millis(80);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().max(1).min(u128::from(u32::MAX)) as u32;
        if unsafe { WaitForSingleObject(input, timeout_ms) } != WAIT_OBJECT_0 {
            break;
        }

        let mut records = [INPUT_RECORD::default(); 128];
        let mut record_count = 0;
        if unsafe {
            PeekConsoleInputW(
                input,
                records.as_mut_ptr(),
                records.len() as u32,
                &mut record_count,
            )
        } == 0
        {
            break;
        }
        let leading_non_keys = records[..record_count as usize]
            .iter()
            .position(|record| {
                if u32::from(record.EventType) != KEY_EVENT {
                    return false;
                }
                let key = unsafe { record.Event.KeyEvent };
                key.bKeyDown != 0 && unsafe { key.uChar.UnicodeChar } != 0
            })
            .unwrap_or(record_count as usize);
        if leading_non_keys > 0 {
            let mut discarded = 0;
            if unsafe {
                ReadConsoleInputW(
                    input,
                    records.as_mut_ptr(),
                    leading_non_keys as u32,
                    &mut discarded,
                )
            } == 0
            {
                break;
            }
            continue;
        }
        if record_count == 0 {
            continue;
        }

        let mut buffer = [0u8; 1024];
        let mut count = 0;
        if unsafe {
            ReadFile(
                input,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut count,
                std::ptr::null_mut(),
            )
        } == 0
        {
            break;
        }
        bytes.extend_from_slice(&buffer[..count as usize]);
        let response = String::from_utf8_lossy(&bytes);
        probe = parse_probe_response(&response);
        if has_device_attributes_reply(&response) {
            break;
        }
    }

    if probe.palette.is_none() {
        probe.palette = query_windows_console_palette().ok().flatten();
    }
    Ok(probe)
}

#[cfg(not(any(unix, windows)))]
fn probe_terminal_impl() -> std::io::Result<TerminalProbe> {
    Ok(TerminalProbe::UNANSWERED)
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
pub(super) fn parse_probe_response(response: &str) -> TerminalProbe {
    let response =
        device_attributes_reply_start(response).map_or(response, |start| &response[..start]);
    let program_status = osc_sequences(response)
        .into_iter()
        .find_map(ProgramStatusSupport::from_reply)
        .unwrap_or(ProgramStatusSupport::Unsupported);
    TerminalProbe {
        palette: parse_palette_response(response),
        program_status,
    }
}

/// Whether `response` holds a primary device attributes reply (`CSI ? Ps c`).
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
pub(super) fn has_device_attributes_reply(response: &str) -> bool {
    device_attributes_reply_start(response).is_some()
}

fn device_attributes_reply_start(response: &str) -> Option<usize> {
    response.match_indices("\x1b[?").find_map(|(start, intro)| {
        response[start + intro.len()..]
            .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == ';')
            .starts_with('c')
            .then_some(start)
    })
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
pub(super) fn osc_sequences(response: &str) -> Vec<&str> {
    let mut sequences = Vec::new();
    let mut rest = response;
    while let Some(start) = rest.find("\x1b]") {
        rest = &rest[start + 2..];
        let bel_end = rest.find('\x07');
        let st_end = rest.find("\x1b\\");
        let Some(end) = earliest_end(bel_end, st_end) else {
            break;
        };
        sequences.push(&rest[..end]);
        rest = &rest[end..];
    }
    sequences
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn earliest_end(bel_end: Option<usize>, st_end: Option<usize>) -> Option<usize> {
    match (bel_end, st_end) {
        (Some(bel), Some(st)) => Some(bel.min(st)),
        (Some(bel), None) => Some(bel),
        (None, Some(st)) => Some(st),
        (None, None) => None,
    }
}
