//! Diff-row palette fills and the chrome package consumed by tool-card render.

use ratatui::{
    style::{Color, Style},
    text::Span,
};
use rho_tools::tool_card::DiffRowKind;

use super::{
    block_foreground, optional_blended, scheme_ansi, AnsiColor, BlockColor, ColorScheme, Palette,
    TerminalPalette, Theme, USER_BACKGROUND_ALPHA,
};

// Diff row wash matches the panel wash strength so syntax stays readable.
const DIFF_ROW_WASH_ALPHA: f32 = USER_BACKGROUND_ALPHA;
// Sign gutter is a stronger blend of the same green/red so +/- reads as a block.
const DIFF_SIGN_GUTTER_ALPHA: f32 = 0.35;

/// Strong gutter + soft wash for one diff side (add or delete).
///
/// Both are absent without sampled RGB so terminals never get harsh named-ANSI
/// backgrounds; sign/text keep role foreground instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DiffSideFill {
    pub(super) gutter: Option<BlockColor>,
    pub(super) wash: Option<BlockColor>,
}

impl DiffSideFill {
    fn from_terminal(terminal: Option<&TerminalPalette>, color: AnsiColor) -> Self {
        Self {
            gutter: optional_blended(terminal, color, DIFF_SIGN_GUTTER_ALPHA),
            wash: optional_blended(terminal, color, DIFF_ROW_WASH_ALPHA),
        }
    }

    fn from_scheme(scheme: &ColorScheme, color: AnsiColor) -> Self {
        Self {
            gutter: Some(scheme_diff_background(
                scheme,
                color,
                DIFF_SIGN_GUTTER_ALPHA,
            )),
            wash: Some(scheme_diff_background(scheme, color, DIFF_ROW_WASH_ALPHA)),
        }
    }
}

/// Layout styles for one diff body row. Theme owns wash policy; render only lays out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct DiffRowChrome {
    /// Content / syntax base (role foreground only; wash applied via [`Self::paint_content`]).
    pub(in crate::tui) text: Style,
    pub(in crate::tui) sign: Style,
    pub(in crate::tui) number: Style,
    pub(in crate::tui) gap: Style,
    pub(in crate::tui) continuation: Style,
    pub(in crate::tui) pad: Style,
    /// Fallback style for empty wrap chunks (includes wash when present).
    pub(in crate::tui) empty_wrap: Style,
    wash: Option<Style>,
}

impl DiffRowChrome {
    /// Syntax roles replace the plain style; re-apply the row wash after.
    pub(in crate::tui) fn paint_content(self, spans: &mut [Span<'static>]) {
        let Some(wash) = self.wash else {
            return;
        };
        for span in spans {
            span.style = span.style.patch(wash);
        }
    }
}

impl Theme {
    /// Chrome for one diff body row: sign gutter, row wash, and padded columns.
    pub(in crate::tui) fn tool_diff_chrome(kind: DiffRowKind) -> DiffRowChrome {
        let palette = Palette::current();
        let text = Self::tool_diff_text(kind);
        let side = match kind {
            DiffRowKind::Added => Some((palette.diff_add, palette.success)),
            DiffRowKind::Removed => Some((palette.diff_del, palette.error)),
            DiffRowKind::Context | DiffRowKind::File | DiffRowKind::Skip | DiffRowKind::Meta => {
                None
            }
        };
        let wash =
            side.and_then(|(fill, _)| fill.wash.map(|block| Style::default().bg(block.color)));
        let sign = match side {
            Some((fill, fallback_fg)) => diff_sign_style(fill.gutter, fallback_fg),
            None => text,
        };
        let washed_text = patch_optional(text, wash);
        DiffRowChrome {
            text,
            sign,
            number: patch_optional(Self::tool_diff_gutter(), wash),
            gap: washed_text,
            continuation: patch_optional(Self::tool_tree(), wash),
            pad: wash.unwrap_or_else(Self::text),
            empty_wrap: washed_text,
            wash,
        }
    }
}

fn patch_optional(base: Style, wash: Option<Style>) -> Style {
    match wash {
        Some(wash) => base.patch(wash),
        None => base,
    }
}

pub(super) fn terminal_diff_fills(
    terminal: Option<&TerminalPalette>,
) -> (DiffSideFill, DiffSideFill) {
    (
        DiffSideFill::from_terminal(terminal, AnsiColor::Green),
        DiffSideFill::from_terminal(terminal, AnsiColor::Red),
    )
}

pub(super) fn scheme_diff_fills(scheme: &ColorScheme) -> (DiffSideFill, DiffSideFill) {
    (
        DiffSideFill::from_scheme(scheme, AnsiColor::Green),
        DiffSideFill::from_scheme(scheme, AnsiColor::Red),
    )
}

fn scheme_diff_background(scheme: &ColorScheme, color: AnsiColor, alpha: f32) -> BlockColor {
    BlockColor::from_rgb(
        scheme
            .background
            .blend_toward(scheme_ansi(scheme, color), alpha),
    )
}

fn diff_sign_style(gutter: Option<BlockColor>, fallback_fg: Color) -> Style {
    match gutter {
        Some(background) => Style::default()
            .fg(block_foreground(background.rgb))
            .bg(background.color),
        None => Style::default().fg(fallback_fg),
    }
}
