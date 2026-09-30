//! Shared presentation, horizontal scrolling, and caret for single-line editors.

use ratatui::layout::Position;
use unicode_segmentation::UnicodeSegmentation;

use super::{
    display_width, styled_line, truncate_one_line, view_composer::ComposerFrame, LineFill, Theme,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum EditorPresentation {
    Plain,
    Masked,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct EditorViewport {
    pub(super) value: String,
    pub(super) cursor_column: usize,
}

/// Project source text and its scalar cursor together, so masked callers never
/// construct a display string or translate the cursor. `start` is the source
/// scalar index of the first visible grapheme; preserve it while the caret fits.
/// A caret inside a grapheme is painted at its end, since terminals cannot
/// address individual scalars. An end-of-value caret needs its own cell.
pub(super) fn editor_viewport(
    value: &str,
    cursor: usize,
    presentation: EditorPresentation,
    width: usize,
    start: &mut usize,
) -> EditorViewport {
    let display_value = match presentation {
        EditorPresentation::Plain => value.replace('\n', " "),
        EditorPresentation::Masked => "•".repeat(value.chars().count()),
    };
    let value = display_value.as_str();
    if width == 0 {
        return EditorViewport {
            value: String::new(),
            cursor_column: 0,
        };
    }
    let mut cursor_column = 0;
    let mut cursor_end = 0;
    let mut cursor_cell_width = 1;
    let mut start_byte = 0;
    let mut start_column = 0;
    let mut chars = 0;
    let mut columns = 0;
    for (index, grapheme) in value.grapheme_indices(true) {
        let grapheme_width = display_width(grapheme);
        if chars <= *start {
            start_byte = index;
            start_column = columns;
        }
        // Only a caret at the grapheme's first scalar must keep that glyph
        // visible. Interior and end-of-value carets still need a spare cell.
        if chars == cursor && grapheme_width <= width {
            cursor_cell_width = grapheme_width.max(1);
        }
        if chars < cursor {
            cursor_column = columns + grapheme_width;
            cursor_end = index + grapheme.len();
        }
        chars += grapheme.chars().count();
        columns += grapheme_width;
    }
    if *start >= chars {
        start_byte = value.len();
        start_column = columns;
    }
    if start_byte > cursor_end {
        start_byte = cursor_end;
        start_column = cursor_column;
    }
    // Backfill newly available space after deletion or a wider resize, rather
    // than leaving the tail of a shortened value stranded in an empty field.
    for (index, grapheme) in value[..start_byte].grapheme_indices(true).rev() {
        let grapheme_width = display_width(grapheme);
        if columns - start_column + grapheme_width >= width {
            break;
        }
        start_byte = index;
        start_column -= grapheme_width;
    }
    cursor_column -= start_column;
    let previous_start = start_byte;
    for (index, grapheme) in value[previous_start..cursor_end].grapheme_indices(true) {
        if cursor_column + cursor_cell_width <= width {
            break;
        }
        cursor_column -= display_width(grapheme);
        start_byte = previous_start + index + grapheme.len();
    }
    *start = value[..start_byte].chars().count();

    let mut end = start_byte;
    let mut used = 0;
    for (index, grapheme) in value[start_byte..].grapheme_indices(true) {
        let grapheme_width = display_width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        used += grapheme_width;
        end = start_byte + index + grapheme.len();
    }
    EditorViewport {
        value: value[start_byte..end].to_owned(),
        cursor_column,
    }
}

/// Overlay chrome consumes only the viewport; editor state and presentation
/// policy stay with the caller.
pub(super) fn editor_frame(prompt: &str, viewport: EditorViewport, width: usize) -> ComposerFrame {
    ComposerFrame::new(
        vec![
            styled_line(
                truncate_one_line(prompt, width),
                width,
                Theme::dim(),
                LineFill::Natural,
            ),
            styled_line(viewport.value, width, Theme::text(), LineFill::Natural),
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
