//! Hover lift paint for click-toggleable tool cards.
//!
//! The whole card stays clickable; hovering lifts the card's text (blended
//! ink, or bold when the theme has no RGB ink) instead of washing the
//! background, so text selection and diff row washes stay legible.

use std::ops::Range;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};

use super::Theme;

/// Lift text ink over the hovered rows, clipped to `area`.
///
/// `rows` indexes rows inside `area` (0-based), already intersected with the
/// visible viewport by the caller. Card rows reuse a handful of ink colors, so
/// lifted colors memoize in a tiny linear cache instead of re-blending per
/// cell; the blend itself only locks the theme palette once per color.
pub(super) fn lift_rows(buffer: &mut Buffer, area: Rect, rows: Range<usize>) {
    let mut memo: Vec<(Color, Option<Color>)> = Vec::new();
    for row in rows {
        let Some(y) = (area.y as usize)
            .checked_add(row)
            .and_then(|y| (y < area.bottom() as usize).then_some(y as u16))
        else {
            continue;
        };
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
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
