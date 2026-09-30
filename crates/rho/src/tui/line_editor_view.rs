//! Shared text viewport and caret for single-line overlay editors.

use ratatui::layout::Position;
use unicode_segmentation::UnicodeSegmentation;

use super::{
    display_width, styled_line, truncate_one_line, view_composer::ComposerFrame, LineFill, Theme,
};

#[derive(Debug, PartialEq, Eq)]
struct EditorViewport<'a> {
    value: &'a str,
    cursor_column: usize,
}

/// Crop on grapheme boundaries, reserving a cell for an end-of-value caret.
/// The editor tracks scalar indices; a caret inside a grapheme is painted at
/// that grapheme's end, since terminals cannot address its individual scalars.
fn editor_viewport(value: &str, cursor: usize, width: usize) -> EditorViewport<'_> {
    if width == 0 {
        return EditorViewport {
            value: "",
            cursor_column: 0,
        };
    }
    let cursor_byte = value
        .char_indices()
        .nth(cursor)
        .map_or(value.len(), |(index, _)| index);
    let mut cursor_column = 0;
    let mut cursor_end = 0;
    for (index, grapheme) in value.grapheme_indices(true) {
        if index >= cursor_byte {
            break;
        }
        cursor_column += display_width(grapheme);
        cursor_end = index + grapheme.len();
    }

    let mut start = 0;
    for (index, grapheme) in value[..cursor_end].grapheme_indices(true) {
        if cursor_column < width {
            break;
        }
        cursor_column = cursor_column.saturating_sub(display_width(grapheme));
        start = index + grapheme.len();
    }

    let mut end = start;
    let mut used = 0;
    for (index, grapheme) in value[start..].grapheme_indices(true) {
        let grapheme_width = display_width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        used += grapheme_width;
        end = start + index + grapheme.len();
    }
    EditorViewport {
        value: &value[start..end],
        cursor_column,
    }
}

/// Derive the displayed value and caret together. Masked callers must supply
/// the mask string, not the underlying secret, so both use the same cell widths.
pub(super) fn editor_frame(
    prompt: &str,
    display_value: &str,
    cursor: usize,
    width: usize,
) -> ComposerFrame {
    let viewport = editor_viewport(display_value, cursor, width);
    ComposerFrame::new(
        vec![
            styled_line(
                truncate_one_line(prompt, width),
                width,
                Theme::dim(),
                LineFill::Natural,
            ),
            styled_line(
                viewport.value.to_owned(),
                width,
                Theme::text(),
                LineFill::Natural,
            ),
        ],
        Position {
            x: viewport.cursor_column as u16,
            y: 1,
        },
    )
}

#[cfg(test)]
#[path = "line_editor_view_tests.rs"]
mod tests;
