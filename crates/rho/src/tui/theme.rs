use std::{collections::HashMap, sync::OnceLock};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

use super::markdown::HeadingLevel;

const USER_BACKGROUND_ALPHA: f32 = 0.10;
const NEUTRAL_TOOL_BACKGROUND_ALPHA: f32 = 0.10;
// Light/dark split for palette-derived chrome. Matches the existing block
// contrast threshold used by block_foreground.
const LIGHT_BACKGROUND_LUMINANCE: f32 = 0.55;
// Dim candidate band: stay muted and readable against the terminal background.
// 0.75 ≈ #c0c0c0 (above this, dark-bg dim collapses into body white).
const DIM_MAX_LUMINANCE_ON_DARK: f32 = 0.75;
// 0.12 ≈ #383838 (below this, dark-bg dim vanishes into the background).
const DIM_MIN_LUMINANCE_ON_DARK: f32 = 0.12;
// 0.45 rejects mid/light bright-black samples on light backgrounds.
const DIM_MAX_LUMINANCE_ON_LIGHT: f32 = 0.45;
// Minimum luminance gap so muted text neither matches the wash nor the body.
const DIM_CONTRAST_MARGIN: f32 = 0.08;

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

    fn luminance(self) -> f32 {
        (0.2126 * f32::from(self.red)
            + 0.7152 * f32::from(self.green)
            + 0.0722 * f32::from(self.blue))
            / 255.0
    }

    /// True when this sample can serve as muted chrome against `background_luminance`.
    fn is_usable_dim(self, background_luminance: f32) -> bool {
        let luminance = self.luminance();
        if is_light_background(background_luminance) {
            luminance + DIM_CONTRAST_MARGIN < background_luminance
                && luminance < DIM_MAX_LUMINANCE_ON_LIGHT
        } else {
            (DIM_MIN_LUMINANCE_ON_DARK..=DIM_MAX_LUMINANCE_ON_DARK).contains(&luminance)
                && luminance >= background_luminance + DIM_CONTRAST_MARGIN
        }
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

    fn dim_foreground(&self) -> Color {
        // Dim chrome comes from ANSI bright black (index 8), never white (index 7).
        let background_luminance = self.background.luminance();
        let fallback = if is_light_background(background_luminance) {
            Color::Black
        } else {
            Color::DarkGray
        };
        self.ansi
            .get(&AnsiColor::BrightBlack)
            .copied()
            .filter(|rgb| rgb.is_usable_dim(background_luminance))
            .map_or(fallback, Rgb::color)
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
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
enum AnsiColor {
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

/// Chromatic colors plus white. Required before a queried palette is accepted.
const REQUIRED_ANSI_COLORS: [AnsiColor; 7] = [
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

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
impl AnsiColor {
    const fn index(self) -> u8 {
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

    const fn color(self) -> Color {
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
}

impl Palette {
    fn current() -> Self {
        let terminal = TERMINAL_PALETTE.get();
        Self::from_terminal(terminal)
    }

    fn from_terminal(terminal: Option<&TerminalPalette>) -> Self {
        Self {
            dim: terminal.map_or(Color::DarkGray, TerminalPalette::dim_foreground),
            accent: AnsiColor::Cyan.color(),
            success: AnsiColor::Green.color(),
            warning: AnsiColor::Yellow.color(),
            error: AnsiColor::Red.color(),
            skill: AnsiColor::Magenta.color(),
            user_background: blended_or_fallback(
                terminal,
                AnsiColor::White,
                USER_BACKGROUND_ALPHA,
                BlockColor::from_color(Color::DarkGray),
            ),
            // Same blend recipe as user prompts today; keep a dedicated field so
            // tool chrome can diverge later without rewriting call sites.
            neutral_tool_background: blended_or_fallback(
                terminal,
                AnsiColor::White,
                NEUTRAL_TOOL_BACKGROUND_ALPHA,
                BlockColor::from_color(Color::DarkGray),
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

    pub(super) fn activity_rail() -> Style {
        let background = Palette::current().neutral_tool_background;
        Style::reset()
            .fg(block_foreground(background.rgb))
            .bg(background.color)
    }

    pub(super) fn jump_to_bottom() -> Style {
        Self::activity_rail().fg(Palette::current().accent)
    }

    pub(super) fn jump_to_bottom_shortcut() -> Style {
        Self::activity_rail().fg(Palette::current().dim)
    }

    pub(super) fn subagent_row(state: super::subagent_panel::SubagentRowState) -> Style {
        use super::subagent_panel::SubagentRowState;
        match state {
            SubagentRowState::Idle => Self::activity_rail(),
            SubagentRowState::Hovered => Self::activity_rail().fg(Palette::current().accent),
            SubagentRowState::Pressed => Style::default()
                .fg(Color::Black)
                .bg(Palette::current().accent)
                .add_modifier(Modifier::BOLD),
        }
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

    pub(super) fn reasoning_output(lines: &mut [Line<'static>]) {
        let reasoning_style = Self::dim();
        for line in lines {
            line.style = reasoning_style
                .patch(line.style)
                .remove_modifier(Modifier::DIM);
            for span in &mut line.spans {
                span.style = reasoning_style
                    .patch(span.style)
                    .remove_modifier(Modifier::DIM);
            }
        }
    }

    pub(super) fn reasoning_input_border(level: rho_providers::reasoning::ReasoningLevel) -> Style {
        let color = match level {
            rho_providers::reasoning::ReasoningLevel::Off => return Theme::dim(),
            rho_providers::reasoning::ReasoningLevel::Minimal => AnsiColor::Blue.color(),
            rho_providers::reasoning::ReasoningLevel::Low => AnsiColor::Cyan.color(),
            rho_providers::reasoning::ReasoningLevel::Medium => AnsiColor::Green.color(),
            rho_providers::reasoning::ReasoningLevel::High => AnsiColor::Yellow.color(),
            rho_providers::reasoning::ReasoningLevel::Xhigh => AnsiColor::Magenta.color(),
            rho_providers::reasoning::ReasoningLevel::Max => AnsiColor::Red.color(),
        };
        Style::default().fg(color)
    }

    pub(super) fn markdown_heading(level: HeadingLevel) -> Style {
        let color = match level {
            HeadingLevel::H1 => AnsiColor::Magenta.color(),
            HeadingLevel::H2 => AnsiColor::Blue.color(),
            HeadingLevel::H3 => AnsiColor::Cyan.color(),
            HeadingLevel::H4 => AnsiColor::Green.color(),
            HeadingLevel::H5 => AnsiColor::Yellow.color(),
            HeadingLevel::H6 => AnsiColor::BrightBlack.color(),
        };
        let style = Style::default()
            .fg(color)
            .remove_modifier(Modifier::UNDERLINED);
        match level {
            HeadingLevel::H1 | HeadingLevel::H2 | HeadingLevel::H3 => {
                style.add_modifier(Modifier::BOLD)
            }
            HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => style,
        }
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

    pub(super) fn command_block() -> Style {
        Self::dim_block(Palette::current().neutral_tool_background)
    }

    /// Status marker color for Call + Children tool cards.
    pub(super) fn tool_marker(status: rho_tools::tool_card::ToolStatus) -> Style {
        use rho_tools::tool_card::ToolStatus;
        match status {
            ToolStatus::Running => Self::accent(),
            ToolStatus::Ok => Self::success(),
            ToolStatus::Error => Self::error(),
            ToolStatus::Interrupted => Self::warning(),
        }
    }

    /// Family color for the tool verb / shell prompt.
    pub(super) fn tool_verb(family: rho_tools::tool_card::ToolFamily) -> Style {
        use rho_tools::tool_card::ToolFamily;
        let palette = Palette::current();
        match family {
            ToolFamily::FileCommand | ToolFamily::FileDiff => Style::default().fg(palette.success),
            ToolFamily::Web => Style::default().fg(AnsiColor::Blue.color()),
            ToolFamily::Skill => Style::default().fg(palette.skill),
            ToolFamily::Form => Style::default().fg(palette.warning),
            ToolFamily::Agent => Self::text(),
            ToolFamily::Default => Self::dim(),
        }
    }

    /// Primary argument style in the header.
    pub(super) fn tool_primary() -> Style {
        Self::text()
    }

    pub(super) fn tool_tree() -> Style {
        Self::dim()
    }

    pub(super) fn tool_meta() -> Style {
        Self::dim()
    }

    pub(super) fn tool_path() -> Style {
        Self::dim()
    }

    pub(super) fn tool_stat_add() -> Style {
        Style::default().fg(Palette::current().success)
    }

    pub(super) fn tool_stat_del() -> Style {
        Style::default().fg(Palette::current().error)
    }

    /// Text color for one diff row. Context stays plain so changes stand out.
    pub(super) fn tool_diff_text(kind: rho_tools::tool_card::DiffRowKind) -> Style {
        use rho_tools::tool_card::DiffRowKind;
        let palette = Palette::current();
        match kind {
            DiffRowKind::Added => Style::default().fg(palette.success),
            DiffRowKind::Removed => Style::default().fg(palette.error),
            DiffRowKind::Context | DiffRowKind::File | DiffRowKind::Skip => Self::text(),
        }
    }

    /// Line-number gutter. The sign carries the change, so numbers stay chrome.
    pub(super) fn tool_diff_gutter() -> Style {
        Self::dim()
    }

    pub(super) fn tool_exit(status: rho_tools::tool_card::ToolStatus) -> Style {
        use rho_tools::tool_card::ToolStatus;
        match status {
            ToolStatus::Ok | ToolStatus::Running => Self::success(),
            ToolStatus::Error | ToolStatus::Interrupted => Self::error(),
        }
    }

    pub(super) fn tool_error_text() -> Style {
        Self::error()
    }

    /// Explicit padding style for tool cards (never sample the marker span).
    pub(super) fn tool_card_padding() -> Style {
        Self::text()
    }

    fn dim_block(background: BlockColor) -> Style {
        Style::default()
            .fg(block_foreground(background.rgb))
            .bg(background.color)
    }
}

fn block_foreground(background: Option<Rgb>) -> Color {
    match background {
        Some(rgb) if is_light_background(rgb.luminance()) => Color::Black,
        Some(_) | None => Color::White,
    }
}

fn is_light_background(luminance: f32) -> bool {
    luminance > LIGHT_BACKGROUND_LUMINANCE
}

fn blended_or_fallback(
    terminal: Option<&TerminalPalette>,
    color: AnsiColor,
    alpha: f32,
    fallback: BlockColor,
) -> BlockColor {
    terminal
        .and_then(|palette| palette.blended_background(color, alpha))
        .unwrap_or(fallback)
}

fn blend_channel(base: u8, overlay: u8, alpha: f32) -> u8 {
    (base as f32 + (overlay as f32 - base as f32) * alpha).round() as u8
}

fn query_terminal_palette() -> Option<TerminalPalette> {
    query_terminal_palette_impl().ok().flatten()
}

#[cfg(windows)]
fn is_native_wezterm() -> bool {
    std::env::var_os("WEZTERM_PANE").is_some()
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
    if is_native_wezterm() {
        // WezTerm's bundled ConPTY does not pass terminal query responses back
        // to native Windows applications. Use the console palette directly.
        return query_windows_console_palette();
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
fn windows_console_palette(color_table: &[u32; 16], attributes: u16) -> TerminalPalette {
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
fn query_terminal_palette_impl() -> std::io::Result<Option<TerminalPalette>> {
    Ok(None)
}

#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
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

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
