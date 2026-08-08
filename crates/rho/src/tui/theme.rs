use std::sync::{Mutex, OnceLock};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

use super::{
    markdown::HeadingLevel,
    theme_scheme::{
        self, is_terminal_theme_id, normalize_theme_id, resolve_fixed_scheme, ColorScheme, Rgb,
        TERMINAL_THEME_ID,
    },
    theme_terminal::{query_terminal_palette, AnsiColor, TerminalPalette},
};

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
// Status/role ink (warning, success, ...) vs surface. Bright yellow on light
// surfaces is the usual failure; require a clear darker/lighter separation.
const ROLE_INK_MARGIN: f32 = 0.22;

/// Host terminal sample captured once at startup.
static TERMINAL_SAMPLE: OnceLock<TerminalPalette> = OnceLock::new();

/// Active theme selection (terminal-matched or a fixed scheme).
static THEME_STATE: Mutex<ThemeState> = Mutex::new(ThemeState::new());

#[derive(Clone, Debug)]
struct ThemeState {
    /// Configured / committed theme id (`terminal`, `one-half-dark`, ...).
    committed_id: String,
    /// Id currently driving colors (may be a live picker preview).
    active_id: String,
    /// Fixed scheme when `active_id` is not terminal.
    active_scheme: Option<ColorScheme>,
    /// Bumps when active colors change so history cache rebuilds.
    generation: u64,
}

impl ThemeState {
    const fn new() -> Self {
        Self {
            committed_id: String::new(),
            active_id: String::new(),
            active_scheme: None,
            generation: 0,
        }
    }
}

impl Rgb {
    fn color(self) -> Color {
        Color::Rgb(self.red, self.green, self.blue)
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
        // Terminal mode keeps named ANSI fallbacks so the host palette paints them.
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

    /// Dim ink for a fixed RGB scheme. Never returns named ANSI fallbacks.
    fn scheme_dim_foreground(scheme: &ColorScheme) -> Color {
        let background = scheme.background;
        let background_luminance = background.luminance();
        let candidates = [
            scheme.ansi[8], // bright black
            scheme.ansi[0], // black
            scheme.foreground,
        ];
        for candidate in candidates {
            if candidate.is_usable_dim(background_luminance) {
                return candidate.color();
            }
        }
        // Last resort: pull foreground toward the surface so chrome stays muted.
        background.blend_toward(scheme.foreground, 0.55).color()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Palette {
    /// Body text. `None` keeps the host default foreground (terminal theme).
    text: Option<Color>,
    /// Full-screen surface. `None` keeps the host default background.
    surface: Option<Color>,
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
        let state = THEME_STATE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match state.active_scheme.as_ref() {
            Some(scheme) => Self::from_scheme(scheme),
            None => Self::from_terminal(TERMINAL_SAMPLE.get()),
        }
    }

    fn from_terminal(terminal: Option<&TerminalPalette>) -> Self {
        let surface = terminal.map(|palette| palette.background);
        Self {
            text: None,
            surface: None,
            dim: terminal.map_or(Color::DarkGray, TerminalPalette::dim_foreground),
            // Prefer sampled RGB when the host answered palette queries so brand,
            // version, and status colors track the real terminal theme instead of
            // generic named ANSI (which often looks "hardcoded").
            accent: role_ink(sampled_or_named(terminal, AnsiColor::Cyan), surface),
            success: role_ink(sampled_or_named(terminal, AnsiColor::Green), surface),
            warning: role_ink(sampled_or_named(terminal, AnsiColor::Yellow), surface),
            error: role_ink(sampled_or_named(terminal, AnsiColor::Red), surface),
            skill: role_ink(sampled_or_named(terminal, AnsiColor::Magenta), surface),
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

    fn from_scheme(scheme: &ColorScheme) -> Self {
        let panel = scheme_panel_background(scheme);
        let surface = scheme.background;
        Self {
            text: Some(scheme.foreground.color()),
            surface: Some(scheme.background.color()),
            dim: TerminalPalette::scheme_dim_foreground(scheme),
            accent: role_ink(scheme.ansi[6].color(), Some(surface)),
            success: role_ink(scheme.ansi[2].color(), Some(surface)),
            warning: role_ink(scheme.ansi[3].color(), Some(surface)),
            error: role_ink(scheme.ansi[1].color(), Some(surface)),
            skill: role_ink(scheme.ansi[5].color(), Some(surface)),
            user_background: panel,
            neutral_tool_background: panel,
        }
    }
}

/// Soft panel wash: blend dark ink into light surfaces, light ink into dark ones.
fn scheme_panel_background(scheme: &ColorScheme) -> BlockColor {
    let background = scheme.background;
    let wash = if is_light_background(background.luminance()) {
        scheme.ansi[0]
    } else {
        scheme.ansi[7]
    };
    BlockColor::from_rgb(background.blend_toward(wash, USER_BACKGROUND_ALPHA))
}

/// Sampled host RGB when available, else the named ANSI fallback.
fn sampled_or_named(terminal: Option<&TerminalPalette>, color: AnsiColor) -> Color {
    terminal
        .and_then(|palette| palette.ansi.get(&color).copied())
        .map(Rgb::color)
        .unwrap_or_else(|| color.color())
}

/// Make role ink readable as text on the UI surface.
///
/// Keeps the hue family (yellow stays warm, red stays red) but pulls bright
/// inks darker on light surfaces and invisible dark inks lighter on dark ones.
/// Named ANSI colors pass through unchanged so the host can still paint them.
fn role_ink(ink: Color, surface: Option<Rgb>) -> Color {
    let Color::Rgb(red, green, blue) = ink else {
        return ink;
    };
    let Some(surface) = surface else {
        return ink;
    };
    let ink = Rgb::new(red, green, blue);
    let surface_luminance = surface.luminance();
    let ink_luminance = ink.luminance();

    if is_light_background(surface_luminance) {
        // Light UI: role text must sit clearly below the surface luminance.
        if ink_luminance + ROLE_INK_MARGIN <= surface_luminance {
            return ink.color();
        }
        return pull_ink_until(ink, Rgb::new(0, 0, 0), |candidate| {
            candidate.luminance() + ROLE_INK_MARGIN <= surface_luminance
        });
    }

    // Dark UI: keep any ink already brighter than the surface; only lift
    // colors that disappear into the background.
    if ink_luminance > surface_luminance {
        return ink.color();
    }
    pull_ink_until(ink, Rgb::new(255, 255, 255), |candidate| {
        candidate.luminance() > surface_luminance
    })
}

fn pull_ink_until(start: Rgb, target: Rgb, acceptable: impl Fn(Rgb) -> bool) -> Color {
    if acceptable(start) {
        return start.color();
    }
    let mut best = start.blend_toward(target, 0.95);
    for alpha in [0.25_f32, 0.4, 0.55, 0.7, 0.85, 0.95] {
        let candidate = start.blend_toward(target, alpha);
        if acceptable(candidate) {
            return candidate.color();
        }
        best = candidate;
    }
    best.color()
}

/// Color for an ANSI role under the active theme.
fn active_ansi_color(color: AnsiColor) -> Color {
    let state = THEME_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(scheme) = state.active_scheme.as_ref() {
        return scheme.ansi[color.index() as usize].color();
    }
    sampled_or_named(TERMINAL_SAMPLE.get(), color)
}

pub(super) struct Theme;

impl Theme {
    pub(super) fn initialize_from_terminal() {
        if let Some(palette) = query_terminal_palette() {
            let _ = TERMINAL_SAMPLE.set(palette);
        }
    }

    /// Apply the committed config theme (startup and after successful apply).
    pub(super) fn apply_committed(id: &str) {
        apply_theme_id(id, /*commit*/ true);
    }

    /// Live preview while browsing the theme picker. Does not change commit.
    pub(super) fn preview(id: &str) {
        apply_theme_id(id, /*commit*/ false);
    }

    /// Abandon a live preview and restore the last committed theme.
    pub(super) fn cancel_preview() {
        let id = {
            let state = THEME_STATE
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.active_id == state.committed_id {
                return;
            }
            normalize_theme_id(&state.committed_id)
        };
        apply_theme_id(&id, /*commit*/ false);
    }

    pub(super) fn committed_id() -> String {
        let id = THEME_STATE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .committed_id
            .clone();
        normalize_theme_id(&id)
    }

    pub(super) fn active_id() -> String {
        let id = THEME_STATE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active_id
            .clone();
        normalize_theme_id(&id)
    }

    /// Cache key: changes when the active palette changes.
    pub(super) fn generation() -> u64 {
        THEME_STATE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .generation
    }

    /// Full-frame / popup base style. Fixed schemes set fg+bg; terminal leaves host defaults.
    pub(super) fn surface() -> Style {
        let palette = Palette::current();
        let mut style = Style::default().remove_modifier(Modifier::UNDERLINED);
        if let Some(fg) = palette.text {
            style = style.fg(fg);
        }
        if let Some(bg) = palette.surface {
            style = style.bg(bg);
        }
        style
    }

    /// Ink that contrasts with a solid fill (RGB under fixed themes).
    pub(super) fn contrasting_ink_on(background: Color) -> Color {
        match background {
            Color::Rgb(red, green, blue) => block_foreground(Some(Rgb::new(red, green, blue))),
            Color::White | Color::Gray | Color::Yellow => Color::Black,
            _ => Color::White,
        }
    }

    pub(super) fn text() -> Style {
        let mut style = Style::default().remove_modifier(Modifier::UNDERLINED);
        if let Some(fg) = Palette::current().text {
            style = style.fg(fg);
        }
        style
    }

    pub(super) fn text_strong() -> Style {
        Self::text().add_modifier(Modifier::BOLD)
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
            SubagentRowState::Pressed => {
                let accent = Palette::current().accent;
                Style::default()
                    .fg(Self::contrasting_ink_on(accent))
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            }
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
            rho_providers::reasoning::ReasoningLevel::Minimal => active_ansi_color(AnsiColor::Blue),
            rho_providers::reasoning::ReasoningLevel::Low => active_ansi_color(AnsiColor::Cyan),
            rho_providers::reasoning::ReasoningLevel::Medium => active_ansi_color(AnsiColor::Green),
            rho_providers::reasoning::ReasoningLevel::High => active_ansi_color(AnsiColor::Yellow),
            rho_providers::reasoning::ReasoningLevel::Xhigh => {
                active_ansi_color(AnsiColor::Magenta)
            }
            rho_providers::reasoning::ReasoningLevel::Max => active_ansi_color(AnsiColor::Red),
        };
        Style::default().fg(color)
    }

    pub(super) fn markdown_heading(level: HeadingLevel) -> Style {
        let color = match level {
            HeadingLevel::H1 => active_ansi_color(AnsiColor::Magenta),
            HeadingLevel::H2 => active_ansi_color(AnsiColor::Blue),
            HeadingLevel::H3 => active_ansi_color(AnsiColor::Cyan),
            HeadingLevel::H4 => active_ansi_color(AnsiColor::Green),
            HeadingLevel::H5 => active_ansi_color(AnsiColor::Yellow),
            HeadingLevel::H6 => active_ansi_color(AnsiColor::BrightBlack),
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
                .fg(Self::contrasting_ink_on(palette.accent))
                .bg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Self::dim_block(palette.neutral_tool_background).add_modifier(Modifier::BOLD)
        }
    }

    pub(super) fn markdown_bold() -> Style {
        Self::text()
            .add_modifier(Modifier::BOLD)
            .remove_modifier(Modifier::UNDERLINED)
    }

    pub(super) fn markdown_italic() -> Style {
        Self::text()
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
            ToolFamily::Web => Style::default().fg(active_ansi_color(AnsiColor::Blue)),
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
            DiffRowKind::Context | DiffRowKind::File | DiffRowKind::Skip | DiffRowKind::Meta => {
                Self::text()
            }
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

fn apply_theme_id(id: &str, commit: bool) {
    let id = normalize_theme_id(id);
    let scheme = if is_terminal_theme_id(&id) {
        None
    } else {
        // Unknown id falls back to terminal so a bad config never blanks the UI.
        resolve_fixed_scheme(&id)
    };
    let resolved_id = if scheme.is_none() {
        TERMINAL_THEME_ID.to_string()
    } else {
        id
    };

    let mut state = THEME_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let changed = state.active_id != resolved_id
        || state.active_scheme.as_ref().map(|s| s.id.as_str())
            != scheme.as_ref().map(|s| s.id.as_str());
    if commit {
        state.committed_id = resolved_id.clone();
    }
    if changed {
        state.active_id = resolved_id;
        state.active_scheme = scheme;
        state.generation = state.generation.saturating_add(1);
    }
}

// Re-export catalog helpers used by the picker and config layers.
pub(super) use theme_scheme::{
    list_themes, theme_display_name, ThemeEntry, TERMINAL_THEME_ID as THEME_TERMINAL_ID,
};

fn block_foreground(background: Option<Rgb>) -> Color {
    let on_light = background.is_some_and(|rgb| is_light_background(rgb.luminance()));
    let state = THEME_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(scheme) = state.active_scheme.as_ref() {
        let fg = scheme.foreground;
        let bg = scheme.background;
        // Prefer the scheme ink that actually contrasts with the panel wash.
        return if on_light {
            if fg.luminance() <= bg.luminance() {
                fg.color()
            } else {
                bg.color()
            }
        } else if fg.luminance() >= bg.luminance() {
            fg.color()
        } else {
            bg.color()
        };
    }
    if on_light {
        Color::Black
    } else {
        Color::White
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

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
