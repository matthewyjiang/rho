//! Host-terminal palette sampling for the interactive TUI.

use std::collections::HashMap;

use super::theme_scheme::Rgb;

/// Chromatic colors plus white. Required before a queried palette is accepted.
pub(super) const REQUIRED_ANSI_COLORS: [AnsiColor; 7] = [
    AnsiColor::Red,
    AnsiColor::Green,
    AnsiColor::Yellow,
    AnsiColor::Blue,
    AnsiColor::Magenta,
    AnsiColor::Cyan,
    AnsiColor::White,
];

/// Colors sampled from the terminal: required set plus optional bright black for dim.
const SAMPLED_ANSI_COLORS: [AnsiColor; 8] = [
    AnsiColor::Red,
    AnsiColor::Green,
    AnsiColor::Yellow,
    AnsiColor::Blue,
    AnsiColor::Magenta,
    AnsiColor::Cyan,
    AnsiColor::White,
    AnsiColor::BrightBlack,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalPalette {
    pub background: Rgb,
    pub ansi: HashMap<AnsiColor, Rgb>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
pub(super) enum AnsiColor {
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    /// ANSI index 7. Most palettes store white here. Blend target only - never dim chrome.
    White,
    /// ANSI index 8 (bright black). Standard muted chrome slot.
    BrightBlack,
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
impl AnsiColor {
    pub(super) const fn index(self) -> u8 {
        match self {
            Self::Red => 1,
            Self::Green => 2,
            Self::Yellow => 3,
            Self::Blue => 4,
            Self::Magenta => 5,
            Self::Cyan => 6,
            Self::White => 7,
            Self::BrightBlack => 8,
        }
    }

    pub(super) const fn color(self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            Self::Red => Color::Red,
            Self::Green => Color::Green,
            Self::Yellow => Color::Yellow,
            Self::Blue => Color::Blue,
            Self::Magenta => Color::Magenta,
            Self::Cyan => Color::Cyan,
            // ratatui has no Color::White. Color::Gray is ANSI SGR 37 (white/grey slot).
            // Color::DarkGray is bright black. Do not treat Gray as muted chrome.
            Self::White => Color::Gray,
            Self::BrightBlack => Color::DarkGray,
        }
    }
}

pub(super) fn write_terminal_palette_queries() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    write_palette_queries(&mut stdout)
}

pub(super) fn collect_terminal_palette_response() -> Option<TerminalPalette> {
    collect_terminal_palette_impl().ok().flatten()
}

fn write_palette_queries(output: &mut impl std::io::Write) -> std::io::Result<()> {
    // White (7) for panel blends; bright black (8) for dim text. Never use 7 as dim.
    output.write_all(b"\x1b]11;?\x1b\\")?;
    for color in SAMPLED_ANSI_COLORS {
        write!(output, "\x1b]4;{};?\x1b\\", color.index())?;
    }
    output.flush()
}

#[cfg(unix)]
fn collect_terminal_palette_impl() -> std::io::Result<Option<TerminalPalette>> {
    use std::io::Read;
    use std::os::fd::AsRawFd;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Ok(None);
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Ok(None);
    }

    // Queries were written at init. First paint already elapsed, so any OSC
    // reply is in the stdin buffer or it never arrived. Do not sleep: waiting
    // after first paint eats keys and paste.
    let mut bytes = Vec::new();
    let mut palette = None;
    let mut handle = stdin.lock();
    loop {
        let mut buffer = [0u8; 1024];
        match handle.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                bytes.extend_from_slice(&buffer[..count]);
                palette = parse_palette_response(&String::from_utf8_lossy(&bytes));
                if palette.is_some() {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => {
                let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
                return Err(error);
            }
        }
    }

    let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    Ok(palette)
}

#[cfg(windows)]
fn is_native_wezterm() -> bool {
    std::env::var_os("WEZTERM_PANE").is_some()
}

#[cfg(windows)]
fn collect_terminal_palette_impl() -> std::io::Result<Option<TerminalPalette>> {
    if is_native_wezterm() {
        // WezTerm's bundled ConPTY does not pass terminal query responses back
        // to native Windows applications. Use the console palette directly.
        return query_windows_console_palette();
    }

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
        return Ok(None);
    }

    let mut original_mode = 0;
    if unsafe { GetConsoleMode(input, &mut original_mode) } == 0 {
        return Ok(None);
    }
    if unsafe { SetConsoleMode(input, original_mode | ENABLE_VIRTUAL_TERMINAL_INPUT) } == 0 {
        return Ok(None);
    }
    let _mode_guard = ConsoleModeGuard {
        handle: input,
        mode: original_mode,
    };

    // Queries were written at init. Drain already-queued OSC only: a blocking
    // wait after first paint eats keys and paste.
    let mut bytes = Vec::new();
    loop {
        if unsafe { WaitForSingleObject(input, 0) } != WAIT_OBJECT_0 {
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
            return Err(std::io::Error::last_os_error());
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
                return Err(std::io::Error::last_os_error());
            }
            continue;
        }
        if record_count == 0 {
            break;
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
            return Err(std::io::Error::last_os_error());
        }
        bytes.extend_from_slice(&buffer[..count as usize]);
        if let Some(palette) = parse_palette_response(&String::from_utf8_lossy(&bytes)) {
            return Ok(Some(palette));
        }
    }

    query_windows_console_palette()
}

#[cfg(windows)]
fn query_windows_console_palette() -> std::io::Result<Option<TerminalPalette>> {
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfoEx, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFOEX, STD_OUTPUT_HANDLE,
    };

    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if output.is_null() || output == -1isize as _ {
        return Ok(None);
    }

    let mut info = CONSOLE_SCREEN_BUFFER_INFOEX {
        cbSize: std::mem::size_of::<CONSOLE_SCREEN_BUFFER_INFOEX>() as u32,
        ..Default::default()
    };
    if unsafe { GetConsoleScreenBufferInfoEx(output, &mut info) } == 0 {
        return Ok(None);
    }

    Ok(Some(windows_console_palette(
        &info.ColorTable,
        info.wAttributes,
    )))
}

#[cfg(any(windows, test))]
pub(super) fn windows_console_palette(color_table: &[u32; 16], attributes: u16) -> TerminalPalette {
    // Win32's table uses attribute-bit order (blue, green, red), not ANSI order.
    const COLORS: [(AnsiColor, usize); 8] = [
        (AnsiColor::Red, 4),
        (AnsiColor::Green, 2),
        (AnsiColor::Yellow, 6),
        (AnsiColor::Blue, 1),
        (AnsiColor::Magenta, 5),
        (AnsiColor::Cyan, 3),
        (AnsiColor::White, 7),
        (AnsiColor::BrightBlack, 8),
    ];
    let ansi = COLORS
        .into_iter()
        .map(|(color, index)| (color, rgb_from_colorref(color_table[index])))
        .collect();
    let background_index = usize::from((attributes >> 4) & 0x0f);

    TerminalPalette {
        background: rgb_from_colorref(color_table[background_index]),
        ansi,
    }
}

#[cfg(any(windows, test))]
fn rgb_from_colorref(color: u32) -> Rgb {
    Rgb::new(color as u8, (color >> 8) as u8, (color >> 16) as u8)
}

#[cfg(not(any(unix, windows)))]
fn collect_terminal_palette_impl() -> std::io::Result<Option<TerminalPalette>> {
    Ok(None)
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
pub(super) fn parse_palette_response(response: &str) -> Option<TerminalPalette> {
    let mut background = None;
    let mut ansi = HashMap::new();

    for sequence in osc_sequences(response) {
        if let Some(color) = sequence.strip_prefix("11;").and_then(parse_rgb_response) {
            background = Some(color);
            continue;
        }

        if let Some(rest) = sequence.strip_prefix("4;") {
            let mut parts = rest.splitn(2, ';');
            let index = parts.next().and_then(|part| part.parse::<u8>().ok());
            let color = parts.next().and_then(parse_rgb_response);
            if let (Some(index), Some(color)) = (index, color) {
                if let Some(ansi_color) = ansi_color_from_index(index) {
                    ansi.insert(ansi_color, color);
                }
            }
        }
    }

    Some(TerminalPalette {
        background: background?,
        ansi,
    })
    // Bright black (index 8) is optional and only improves dim text when present.
    .filter(|palette| {
        REQUIRED_ANSI_COLORS
            .into_iter()
            .all(|color| palette.ansi.contains_key(&color))
    })
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn osc_sequences(response: &str) -> Vec<&str> {
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

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn parse_rgb_response(response: &str) -> Option<Rgb> {
    let rgb = response.strip_prefix("rgb:")?;
    let mut components = rgb.split('/');
    Some(Rgb::new(
        parse_xterm_component(components.next()?)?,
        parse_xterm_component(components.next()?)?,
        parse_xterm_component(components.next()?)?,
    ))
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn parse_xterm_component(component: &str) -> Option<u8> {
    let value = u16::from_str_radix(component, 16).ok()?;
    let max = (1u32 << (component.len() * 4)) - 1;
    Some(((value as u32 * 255 + max / 2) / max) as u8)
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
fn ansi_color_from_index(index: u8) -> Option<AnsiColor> {
    match index {
        1 => Some(AnsiColor::Red),
        2 => Some(AnsiColor::Green),
        3 => Some(AnsiColor::Yellow),
        4 => Some(AnsiColor::Blue),
        5 => Some(AnsiColor::Magenta),
        6 => Some(AnsiColor::Cyan),
        7 => Some(AnsiColor::White),
        8 => Some(AnsiColor::BrightBlack),
        _ => None,
    }
}
