use std::{collections::HashMap, sync::OnceLock};

use ratatui::style::{Color, Modifier, Style};

const USER_BACKGROUND_ALPHA: f32 = 0.10;
const TOOL_BACKGROUND_ALPHA: f32 = 0.16;
const FALLBACK_BACKGROUND: Rgb = Rgb::new(12, 12, 12);

static TERMINAL_PALETTE: OnceLock<TerminalPalette> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    fn color(self) -> Color {
        Color::Rgb(self.red, self.green, self.blue)
    }

    fn blend_toward(self, overlay: Self, alpha: f32) -> Self {
        Self::new(
            blend_channel(self.red, overlay.red, alpha),
            blend_channel(self.green, overlay.green, alpha),
            blend_channel(self.blue, overlay.blue, alpha),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalPalette {
    background: Rgb,
    ansi: HashMap<AnsiColor, Rgb>,
}

impl TerminalPalette {
    fn blended_background(&self, color: AnsiColor, alpha: f32) -> Option<BlockColor> {
        self.ansi.get(&color).map(|ansi| {
            let rgb = self.background.blend_toward(*ansi, alpha);
            BlockColor::from_rgb(rgb)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlockColor {
    color: Color,
    rgb: Option<Rgb>,
}

impl BlockColor {
    fn from_rgb(rgb: Rgb) -> Self {
        Self {
            color: rgb.color(),
            rgb: Some(rgb),
        }
    }

    const fn from_color(color: Color) -> Self {
        Self { color, rgb: None }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
enum AnsiColor {
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
}

#[cfg_attr(not(unix), allow(dead_code))]
impl AnsiColor {
    const fn index(self) -> u8 {
        match self {
            Self::Red => 1,
            Self::Green => 2,
            Self::Yellow => 3,
            Self::Blue => 4,
            Self::Magenta => 5,
            Self::Cyan => 6,
            Self::Gray => 7,
        }
    }

    const fn color(self) -> Color {
        match self {
            Self::Red => Color::Red,
            Self::Green => Color::Green,
            Self::Yellow => Color::Yellow,
            Self::Blue => Color::Blue,
            Self::Magenta => Color::Magenta,
            Self::Cyan => Color::Cyan,
            Self::Gray => Color::Gray,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Palette {
    dim: Color,
    accent: Color,
    success: Color,
    warning: Color,
    error: Color,
    skill: Color,
    user_background: BlockColor,
    neutral_tool_background: BlockColor,
    success_tool_background: BlockColor,
    failure_tool_background: BlockColor,
    skill_tool_background: BlockColor,
}

impl Palette {
    fn current() -> Self {
        let terminal = TERMINAL_PALETTE.get();
        Self {
            dim: Color::DarkGray,
            accent: AnsiColor::Cyan.color(),
            success: AnsiColor::Green.color(),
            warning: AnsiColor::Yellow.color(),
            error: AnsiColor::Red.color(),
            skill: AnsiColor::Magenta.color(),
            user_background: blended_or_fallback(terminal, AnsiColor::Gray, USER_BACKGROUND_ALPHA),
            neutral_tool_background: blended_or_fallback(
                terminal,
                AnsiColor::Gray,
                USER_BACKGROUND_ALPHA,
            ),
            success_tool_background: blended_or_fallback(
                terminal,
                AnsiColor::Green,
                TOOL_BACKGROUND_ALPHA,
            ),
            failure_tool_background: blended_or_fallback(
                terminal,
                AnsiColor::Red,
                TOOL_BACKGROUND_ALPHA,
            ),
            skill_tool_background: blended_or_fallback(
                terminal,
                AnsiColor::Magenta,
                TOOL_BACKGROUND_ALPHA,
            ),
        }
    }
}

pub(super) struct Theme;

impl Theme {
    pub(super) fn initialize_from_terminal() {
        if let Some(palette) = query_terminal_palette() {
            let _ = TERMINAL_PALETTE.set(palette);
        }
    }

    pub(super) fn text() -> Style {
        Style::default().remove_modifier(Modifier::UNDERLINED)
    }

    pub(super) fn text_strong() -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub(super) fn dim() -> Style {
        Style::default().fg(Palette::current().dim)
    }

    pub(super) fn dim_italic() -> Style {
        Self::dim().add_modifier(Modifier::ITALIC)
    }

    pub(super) fn accent() -> Style {
        Style::default().fg(Palette::current().accent)
    }

    pub(super) fn brand() -> Style {
        Self::accent().add_modifier(Modifier::BOLD)
    }

    pub(super) fn success() -> Style {
        Style::default()
            .fg(Palette::current().success)
            .add_modifier(Modifier::BOLD)
    }

    pub(super) fn warning() -> Style {
        Style::default()
            .fg(Palette::current().warning)
            .add_modifier(Modifier::BOLD)
    }

    pub(super) fn error() -> Style {
        Style::default()
            .fg(Palette::current().error)
            .add_modifier(Modifier::BOLD)
    }

    pub(super) fn input_prompt() -> Style {
        Style::default()
            .fg(Palette::current().accent)
            .add_modifier(Modifier::BOLD)
    }

    pub(super) fn user_message() -> Style {
        Self::dim_block(Palette::current().user_background)
    }

    pub(super) fn reasoning_input_border(level: crate::reasoning::ReasoningLevel) -> Style {
        let color = match level {
            crate::reasoning::ReasoningLevel::Off => return Theme::dim(),
            crate::reasoning::ReasoningLevel::Minimal => AnsiColor::Blue.color(),
            crate::reasoning::ReasoningLevel::Low => AnsiColor::Cyan.color(),
            crate::reasoning::ReasoningLevel::Medium => AnsiColor::Green.color(),
            crate::reasoning::ReasoningLevel::High => AnsiColor::Yellow.color(),
            crate::reasoning::ReasoningLevel::Xhigh => AnsiColor::Magenta.color(),
            crate::reasoning::ReasoningLevel::Max => AnsiColor::Red.color(),
        };
        Style::default().fg(color)
    }

    pub(super) fn markdown_inline_code() -> Style {
        Style::default()
            .fg(Palette::current().warning)
            .remove_modifier(Modifier::UNDERLINED)
    }

    pub(super) fn markdown_code_block() -> Style {
        Style::default()
            .fg(Palette::current().accent)
            .remove_modifier(Modifier::UNDERLINED)
    }

    pub(super) fn markdown_code_copy_button(hovered: bool) -> Style {
        let palette = Palette::current();
        if hovered {
            Style::default()
                .fg(Color::Black)
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Self::dim_block(palette.neutral_tool_background).add_modifier(Modifier::BOLD)
        }
    }

    pub(super) fn markdown_bold() -> Style {
        Style::default()
            .add_modifier(Modifier::BOLD)
            .remove_modifier(Modifier::UNDERLINED)
    }

    pub(super) fn markdown_italic() -> Style {
        Style::default()
            .add_modifier(Modifier::ITALIC)
            .remove_modifier(Modifier::UNDERLINED)
    }

    pub(super) fn markdown_link() -> Style {
        Style::default()
            .fg(Palette::current().accent)
            .add_modifier(Modifier::UNDERLINED)
    }

    pub(super) fn diff_addition(base: Style) -> Style {
        base.fg(Palette::current().success)
    }

    pub(super) fn diff_removal(base: Style) -> Style {
        base.fg(Palette::current().error)
    }

    pub(super) fn tool_default() -> ToolStyle {
        let palette = Palette::current();
        ToolStyle::new(
            Self::dim_block(palette.neutral_tool_background),
            Self::dim_block(palette.failure_tool_background),
        )
    }

    pub(super) fn tool_file_or_command() -> ToolStyle {
        let palette = Palette::current();
        ToolStyle::new(
            Self::dim_block(palette.success_tool_background),
            Self::dim_block(palette.failure_tool_background),
        )
    }

    pub(super) fn tool_skill() -> ToolStyle {
        let palette = Palette::current();
        ToolStyle::new(
            Self::dim_block(palette.skill_tool_background),
            Self::dim_block(palette.failure_tool_background),
        )
    }

    fn dim_block(background: BlockColor) -> Style {
        Style::default()
            .fg(block_foreground(background.rgb))
            .bg(background.color)
    }
}

fn block_foreground(background: Option<Rgb>) -> Color {
    match background {
        Some(rgb) if relative_luminance(rgb.red, rgb.green, rgb.blue) > 0.55 => Color::Black,
        Some(_) | None => Color::White,
    }
}

fn relative_luminance(red: u8, green: u8, blue: u8) -> f32 {
    (0.2126 * f32::from(red) + 0.7152 * f32::from(green) + 0.0722 * f32::from(blue)) / 255.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ToolStyle {
    success: Style,
    failure: Style,
}

impl ToolStyle {
    const fn new(success: Style, failure: Style) -> Self {
        Self { success, failure }
    }

    pub(super) fn for_result(self, ok: bool) -> Style {
        if ok {
            self.success
        } else {
            self.failure
        }
    }
}

fn blended_or_fallback(
    terminal: Option<&TerminalPalette>,
    color: AnsiColor,
    alpha: f32,
) -> BlockColor {
    terminal
        .and_then(|palette| palette.blended_background(color, alpha))
        .unwrap_or_else(|| {
            let fallback_ansi = match color {
                AnsiColor::Red => Rgb::new(205, 49, 49),
                AnsiColor::Green => Rgb::new(13, 188, 121),
                AnsiColor::Yellow => Rgb::new(229, 229, 16),
                AnsiColor::Blue => Rgb::new(36, 114, 200),
                AnsiColor::Magenta => Rgb::new(188, 63, 188),
                AnsiColor::Cyan => Rgb::new(17, 168, 205),
                AnsiColor::Gray => Rgb::new(204, 204, 204),
            };
            BlockColor::from_rgb(FALLBACK_BACKGROUND.blend_toward(fallback_ansi, alpha))
        })
}

fn blend_channel(base: u8, overlay: u8, alpha: f32) -> u8 {
    (base as f32 + (overlay as f32 - base as f32) * alpha).round() as u8
}

fn query_terminal_palette() -> Option<TerminalPalette> {
    query_terminal_palette_impl().ok().flatten()
}

fn write_palette_queries(output: &mut impl std::io::Write) -> std::io::Result<()> {
    const COLORS: [AnsiColor; 7] = [
        AnsiColor::Red,
        AnsiColor::Green,
        AnsiColor::Yellow,
        AnsiColor::Blue,
        AnsiColor::Magenta,
        AnsiColor::Cyan,
        AnsiColor::Gray,
    ];

    output.write_all(b"\x1b]11;?\x1b\\")?;
    for color in COLORS {
        write!(output, "\x1b]4;{};?\x1b\\", color.index())?;
    }
    output.flush()
}

#[cfg(unix)]
fn query_terminal_palette_impl() -> std::io::Result<Option<TerminalPalette>> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    let mut stdout = std::io::stdout();
    write_palette_queries(&mut stdout)?;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Ok(None);
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Ok(None);
    }

    let mut bytes = Vec::new();
    let mut palette = None;
    let deadline = Instant::now() + Duration::from_millis(80);
    let mut handle = stdin.lock();
    while Instant::now() < deadline && palette.is_none() {
        let mut buffer = [0u8; 1024];
        match handle.read(&mut buffer) {
            Ok(0) => std::thread::sleep(Duration::from_millis(2)),
            Ok(count) => {
                bytes.extend_from_slice(&buffer[..count]);
                palette = parse_palette_response(&String::from_utf8_lossy(&bytes));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2));
            }
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
fn query_terminal_palette_impl() -> std::io::Result<Option<TerminalPalette>> {
    use std::io::{stdout, Write};
    use std::ptr::null_mut;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_INPUT,
        STD_INPUT_HANDLE,
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

    let mut output = stdout();
    write_palette_queries(&mut output)?;

    let mut bytes = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(80);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().max(1).min(u128::from(u32::MAX)) as u32;
        if unsafe { WaitForSingleObject(input, timeout_ms) } != WAIT_OBJECT_0 {
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
                null_mut(),
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

    Ok(None)
}

#[cfg(not(any(unix, windows)))]
fn query_terminal_palette_impl() -> std::io::Result<Option<TerminalPalette>> {
    Ok(None)
}

#[cfg_attr(not(unix), allow(dead_code))]
fn parse_palette_response(response: &str) -> Option<TerminalPalette> {
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
    .filter(|palette| palette.ansi.len() >= 7)
}

#[cfg_attr(not(unix), allow(dead_code))]
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

#[cfg_attr(not(unix), allow(dead_code))]
fn earliest_end(bel_end: Option<usize>, st_end: Option<usize>) -> Option<usize> {
    match (bel_end, st_end) {
        (Some(bel), Some(st)) => Some(bel.min(st)),
        (Some(bel), None) => Some(bel),
        (None, Some(st)) => Some(st),
        (None, None) => None,
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn parse_rgb_response(response: &str) -> Option<Rgb> {
    let rgb = response.strip_prefix("rgb:")?;
    let mut components = rgb.split('/');
    Some(Rgb::new(
        parse_xterm_component(components.next()?)?,
        parse_xterm_component(components.next()?)?,
        parse_xterm_component(components.next()?)?,
    ))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn parse_xterm_component(component: &str) -> Option<u8> {
    let value = u16::from_str_radix(component, 16).ok()?;
    let max = (1u32 << (component.len() * 4)) - 1;
    Some(((value as u32 * 255 + max / 2) / max) as u8)
}

#[cfg_attr(not(unix), allow(dead_code))]
fn ansi_color_from_index(index: u8) -> Option<AnsiColor> {
    match index {
        1 => Some(AnsiColor::Red),
        2 => Some(AnsiColor::Green),
        3 => Some(AnsiColor::Yellow),
        4 => Some(AnsiColor::Blue),
        5 => Some(AnsiColor::Magenta),
        6 => Some(AnsiColor::Cyan),
        7 => Some(AnsiColor::Gray),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_osc_palette_response() {
        let response = "\x1b]11;rgb:0000/0000/0000\x1b\\\x1b]4;1;rgb:ffff/0000/0000\x1b\\\x1b]4;2;rgb:0000/ffff/0000\x1b\\\x1b]4;3;rgb:ffff/ffff/0000\x1b\\\x1b]4;4;rgb:0000/0000/ffff\x1b\\\x1b]4;5;rgb:ffff/0000/ffff\x1b\\\x1b]4;6;rgb:0000/ffff/ffff\x1b\\\x1b]4;7;rgb:ffff/ffff/ffff\x1b\\";

        let palette = parse_palette_response(response).expect("palette");

        assert_eq!(palette.background, Rgb::new(0, 0, 0));
        assert_eq!(palette.ansi[&AnsiColor::Red], Rgb::new(255, 0, 0));
    }

    #[test]
    fn chooses_dark_block_foreground_for_light_resolved_backgrounds() {
        assert_eq!(
            block_foreground(Some(Rgb::new(240, 240, 240))),
            Color::Black
        );
        assert_eq!(block_foreground(Some(Rgb::new(20, 20, 20))), Color::White);
        assert_eq!(block_foreground(None), Color::White);
    }

    #[test]
    fn blends_toward_terminal_ansi_color() {
        let base = Rgb::new(10, 10, 10);
        let green = Rgb::new(10, 110, 10);

        assert_eq!(base.blend_toward(green, 0.16), Rgb::new(10, 26, 10));
    }

    #[test]
    fn resolved_ansi_background_keeps_rgb_for_foreground_contrast() {
        let palette = TerminalPalette {
            background: Rgb::new(255, 255, 255),
            ansi: HashMap::from([(AnsiColor::Gray, Rgb::new(240, 240, 240))]),
        };

        let background = palette
            .blended_background(AnsiColor::Gray, USER_BACKGROUND_ALPHA)
            .expect("resolved background");

        assert_eq!(background.color, Color::Rgb(254, 254, 254));
        assert_eq!(block_foreground(background.rgb), Color::Black);
    }
}
