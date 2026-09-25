//! Hover lift paint: tool cards, composer choices, palette and picker rows.
//!
//! The whole target stays clickable; hovering lifts its text (blended ink, or
//! bold when the theme has no RGB ink) instead of washing the background, so
//! text selection, diff row washes, and per-span colors stay legible.

use std::ops::Range;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};
use rho_sdk::ToolCallId;

use super::Theme;

/// Which toggleable tool card a history line belongs to.
#[derive(Clone, Debug)]
pub(super) enum ToolCardTarget {
    Transcript(usize),
    Preview(usize),
    Running(ToolCallId),
}

/// A toggleable tool card under the pointer, with the absolute history lines
/// covering the whole clickable card.
pub(super) struct ToolCardHit {
    pub(super) target: ToolCardTarget,
    pub(super) lines: Range<usize>,
}

/// Lift text ink over the hovered content lines, clipped to the visible
/// viewport in `area`.
///
/// `first_visible_line` is the content-line index of `area`'s first row, the
/// same contract as [`super::highlight_selection`]. Card rows reuse a handful
/// of ink colors, so lifted colors memoize in a tiny linear cache instead of
/// re-blending per cell; the blend itself only locks the theme palette once
/// per color.
pub(super) fn lift_lines(
    buffer: &mut Buffer,
    area: Rect,
    first_visible_line: usize,
    lines: Range<usize>,
) {
    lift_cells(buffer, area, first_visible_line, lines, 0..usize::MAX);
}

/// [`lift_lines`] restricted to `columns` (relative to `area.x`). Each cell
/// keeps its own hue, so badges, warnings, and dim details on a hovered row
/// stay distinguishable while the whole target reads as lifted.
pub(super) fn lift_cells(
    buffer: &mut Buffer,
    area: Rect,
    first_visible_line: usize,
    lines: Range<usize>,
    columns: Range<usize>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let visible_end = first_visible_line.saturating_add(area.height as usize);
    let start = lines.start.max(first_visible_line);
    let end = lines.end.min(visible_end);
    if start >= end {
        return;
    }
    let mut memo: Vec<(Color, Option<Color>)> = Vec::new();
    for row in start - first_visible_line..end - first_visible_line {
        let Some(y) = (area.y as usize)
            .checked_add(row)
            .and_then(|y| (y < area.bottom() as usize).then_some(y as u16))
        else {
            continue;
        };
        let left = usize::from(area.x).saturating_add(columns.start);
        let right = usize::from(area.x)
            .saturating_add(columns.end)
            .min(usize::from(area.right()));
        for x in left..right {
            // Bounded by `area.right()`, so it fits u16.
            let cell = &mut buffer[(x as u16, y)];
            let fg = cell.fg;
            let lifted = match memo.iter().find(|(seen, _)| *seen == fg) {
                Some((_, lifted)) => *lifted,
                None => {
                    let lifted = Theme::hover_lifted(fg);
                    memo.push((fg, lifted));
                    lifted
                }
            };
            match lifted {
                Some(color) => cell.fg = color,
                None => cell.modifier = cell.modifier.union(Modifier::BOLD),
            }
        }
    }
}
