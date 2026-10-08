//! Host-terminal palette queries, parsing, and platform fallback.

use std::collections::HashMap;

use super::terminal_probe::osc_sequences;
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

/// GitHub-dark well used by the docs PTY proof plate (`SvgPalette::github_dark`).
///
/// Matrix PTYs do not answer OSC palette queries. The proof-plate launcher
/// sets `RHO_TUI_MATRIX_PALETTE=github-dark` so that capture gets RGB
/// add/remove washes. Other PTY scenarios keep unsamped terminal chrome.
/// Only green/red are filled so other roles stay named ANSI.
pub(super) fn matrix_fixture_palette() -> TerminalPalette {
    TerminalPalette {
        background: Rgb::new(0x0d, 0x11, 0x17),
        ansi: HashMap::from([
            (AnsiColor::Green, Rgb::new(0x3f, 0xb9, 0x50)),
            (AnsiColor::Red, Rgb::new(0xff, 0x7b, 0x72)),
        ]),
    }
}

pub(super) fn write_palette_queries(output: &mut impl std::io::Write) -> std::io::Result<()> {
    // White (7) for panel blends; bright black (8) for dim text. Never use 7 as dim.
    output.write_all(b"\x1b]11;?\x1b\\")?;
    for color in SAMPLED_ANSI_COLORS {
        write!(output, "\x1b]4;{};?\x1b\\", color.index())?;
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn query_windows_console_palette() -> std::io::Result<Option<TerminalPalette>> {
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
