//! Diff-row wash and the chrome package consumed by tool-card render.
//!
//! Added/removed rows get a soft green/red wash. Change kind on the sign is
//! foreground only (`+`/`-`); content stays base text plus syntax roles.
//! Theme owns wash + sign; render derives each column with [`DiffRowChrome::washed`].

use ratatui::{style::Style, text::Span};
use rho_tools::tool_card::DiffRowKind;

use super::{
    optional_blended, scheme_ansi, AnsiColor, BlockColor, ColorScheme, Palette, TerminalPalette,
    Theme, USER_BACKGROUND_ALPHA,
};

// Diff row wash matches the panel wash strength so syntax stays readable.
const DIFF_ROW_WASH_ALPHA: f32 = USER_BACKGROUND_ALPHA;

/// Theme facts for one diff body row: content base, sign role, optional wash.
///
/// Render patches column bases through [`Self::washed`]; content syntax is
/// washed after highlight via [`Self::paint_content`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct DiffRowChrome {
    /// Content / syntax base (role foreground only).
    pub(in crate::tui) text: Style,
    /// `+`/`-` role foreground (+ wash when present).
    pub(in crate::tui) sign: Style,
    wash: Option<Style>,
}

impl DiffRowChrome {
    /// Apply the row wash to a column's base style.
    pub(in crate::tui) fn washed(self, base: Style) -> Style {
        match self.wash {
            Some(wash) => base.patch(wash),
            None => base,
        }
    }

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
    /// Chrome for one diff body row: fg `+`/`-`, soft row wash, plain content.
    pub(in crate::tui) fn tool_diff_chrome(kind: DiffRowKind) -> DiffRowChrome {
        let palette = Palette::current();
        let text = Self::text();
        let (wash_fill, sign_fg) = match kind {
            DiffRowKind::Added => (palette.diff_add_wash, Some(palette.success)),
            DiffRowKind::Removed => (palette.diff_del_wash, Some(palette.error)),
            DiffRowKind::Context | DiffRowKind::File | DiffRowKind::Skip | DiffRowKind::Meta => {
                (None, None)
            }
        };
        let wash = wash_fill.map(|block| Style::default().bg(block.color));
        // Sign is role foreground only - the wash carries add/remove, not a solid gutter.
        let sign = match (sign_fg, wash) {
            (Some(fg), Some(wash)) => Style::default().fg(fg).patch(wash),
            (Some(fg), None) => Style::default().fg(fg),
            (None, _) => text,
        };
        DiffRowChrome { text, sign, wash }
    }
}

pub(super) fn terminal_diff_washes(
    terminal: Option<&TerminalPalette>,
) -> (Option<BlockColor>, Option<BlockColor>) {
    (
        optional_blended(terminal, AnsiColor::Green, DIFF_ROW_WASH_ALPHA),
        optional_blended(terminal, AnsiColor::Red, DIFF_ROW_WASH_ALPHA),
    )
}

pub(super) fn scheme_diff_washes(scheme: &ColorScheme) -> (Option<BlockColor>, Option<BlockColor>) {
    (
        Some(scheme_diff_background(scheme, AnsiColor::Green)),
        Some(scheme_diff_background(scheme, AnsiColor::Red)),
    )
}

fn scheme_diff_background(scheme: &ColorScheme, color: AnsiColor) -> BlockColor {
    BlockColor::from_rgb(
        scheme
            .background
            .blend_toward(scheme_ansi(scheme, color), DIFF_ROW_WASH_ALPHA),
    )
}
