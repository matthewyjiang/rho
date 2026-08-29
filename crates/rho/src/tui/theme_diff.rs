//! Diff-row wash and the chrome package consumed by tool-card render.
//!
//! Sampled surfaces (terminal RGB or a named scheme) mix toward green/red so
//! the row wash is actually visible on that background. The sign is add/remove
//! foreground. Unhighlighted content sits on the wash, or uses that foreground
//! when no RGB wash exists. Syntax roles replace fg and keep the wash.

use ratatui::{style::Style, text::Span};
use rho_tools::tool_card::DiffRowKind;

use super::{
    is_light_background, scheme_ansi, AnsiColor, BlockColor, ColorScheme, Palette, Rgb,
    TerminalPalette, Theme, USER_BACKGROUND_ALPHA,
};

/// VS Code GitHub Dark `diffEditor.insertedLineBackground` is `#2ea04326`
/// (0x26 / 255). Chromatic mixes never go weaker than that overlay.
const GITHUB_DIFF_LINE_ALPHA: f32 = 0x26 as f32 / 255.0;

/// Cap so a tint close to the surface cannot become a solid role fill.
/// Builtin schemes compute ~0.13–0.18; if a sampled theme hits this, raise it.
const MAX_DIFF_WASH_ALPHA: f32 = 0.35;

/// Theme facts for one diff body row: sign role and optional wash.
///
/// Render patches column bases through [`Self::washed`]; content syntax is
/// washed after highlight via [`Self::paint_content`]. Unhighlighted content
/// uses [`Self::plain`]: body text on the wash, or add/remove fg when there
/// is no wash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct DiffRowChrome {
    /// `+`/`-` role foreground (+ wash when present).
    pub(in crate::tui) sign: Style,
    wash: Option<Style>,
}

impl DiffRowChrome {
    fn new(sign_base: Style, wash: Option<Style>) -> Self {
        let chrome = Self {
            sign: sign_base,
            wash,
        };
        Self {
            sign: chrome.washed(sign_base),
            wash,
        }
    }

    /// Apply the row wash to a column's base style.
    pub(in crate::tui) fn washed(self, base: Style) -> Style {
        match self.wash {
            Some(wash) => base.patch(wash),
            None => base,
        }
    }

    /// Unhighlighted content: body text on the row wash, or add/remove fg
    /// when the theme has no RGB wash (default `terminal` without a sample).
    ///
    /// Syntax roles patch onto this so the wash is not dropped.
    pub(in crate::tui) fn plain(self) -> Style {
        if self.wash.is_some() {
            self.washed(Theme::text())
        } else {
            self.sign
        }
    }

    /// Syntax roles replace the plain style; re-apply the row wash after.
    pub(in crate::tui) fn paint_content(self, spans: &mut [Span<'static>]) {
        if self.wash.is_none() {
            return;
        }
        for span in spans {
            span.style = self.washed(span.style);
        }
    }
}

impl Theme {
    /// Chrome for one diff body row: fg `+`/`-`, soft row wash.
    pub(in crate::tui) fn tool_diff_chrome(kind: DiffRowKind) -> DiffRowChrome {
        let palette = Palette::current();
        let (wash_fill, sign_fg) = match kind {
            DiffRowKind::Added => (palette.diff_add_wash, Some(palette.success)),
            DiffRowKind::Removed => (palette.diff_del_wash, Some(palette.error)),
            DiffRowKind::Context | DiffRowKind::File | DiffRowKind::Skip | DiffRowKind::Meta => {
                (None, None)
            }
        };
        let wash = wash_fill.map(|block| Style::default().bg(block.color));
        let sign_base = sign_fg.map_or(Self::text(), |fg| Style::default().fg(fg));
        DiffRowChrome::new(sign_base, wash)
    }
}

pub(super) fn terminal_diff_washes(
    terminal: Option<&TerminalPalette>,
) -> (Option<BlockColor>, Option<BlockColor>) {
    (
        terminal.and_then(|terminal| sampled_diff_wash(terminal, AnsiColor::Green)),
        terminal.and_then(|terminal| sampled_diff_wash(terminal, AnsiColor::Red)),
    )
}

pub(super) fn scheme_diff_washes(scheme: &ColorScheme) -> (Option<BlockColor>, Option<BlockColor>) {
    (
        Some(visible_diff_wash(
            scheme.background,
            scheme_ansi(scheme, AnsiColor::Green),
        )),
        Some(visible_diff_wash(
            scheme.background,
            scheme_ansi(scheme, AnsiColor::Red),
        )),
    )
}

fn sampled_diff_wash(terminal: &TerminalPalette, color: AnsiColor) -> Option<BlockColor> {
    let tint = *terminal.ansi.get(&color)?;
    Some(visible_diff_wash(terminal.background, tint))
}

/// Mix `tint` into `background` until the wash is as separated as the panel
/// wash on this surface, and never weaker than GitHub's 15% line overlay.
fn visible_diff_wash(background: Rgb, tint: Rgb) -> BlockColor {
    BlockColor::from_rgb(background.blend_toward(tint, diff_wash_alpha(background, tint)))
}

fn diff_wash_alpha(background: Rgb, tint: Rgb) -> f32 {
    let panel_ink = if is_light_background(background.luminance()) {
        Rgb::new(0, 0, 0)
    } else {
        Rgb::new(255, 255, 255)
    };
    let panel = background.blend_toward(panel_ink, USER_BACKGROUND_ALPHA);
    let target_delta = (panel.luminance() - background.luminance()).abs();
    let tint_span = (tint.luminance() - background.luminance()).abs();
    let matched = if tint_span == 0.0 {
        GITHUB_DIFF_LINE_ALPHA
    } else {
        target_delta / tint_span
    };
    matched.clamp(GITHUB_DIFF_LINE_ALPHA, MAX_DIFF_WASH_ALPHA)
}
