//! History text-selection pointer behavior.

use std::time::{Duration, Instant};

use anyhow::Result;
use unicode_width::UnicodeWidthChar;

use super::{STARTUP, STREAM};
use crate::{harness::WaitTimeout, keys::MouseButton, scenario::Step, PtyHarness};

const DRAG_WIDTH: u16 = 8;

/// Display columns occupied by the UTF-8 prefix `line[..byte_offset]`.
fn display_column_at_byte(line: &str, byte_offset: usize) -> u16 {
    line.get(..byte_offset)
        .unwrap_or("")
        .chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum::<usize>() as u16
}

/// Finds the row and 0-based display column where `needle` is rendered on screen.
fn screen_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate() {
        if let Some(byte_offset) = line.find(needle) {
            return Ok((row as u16, display_column_at_byte(line, byte_offset)));
        }
    }
    anyhow::bail!("'{needle}' not found:\n{}", harness.screen().debug_dump());
}

/// Polls until every column in `column..column + cells` of `row` renders with
/// the inverse attribute, failing with `what` and a screen dump on timeout.
fn wait_for_row_highlight(
    harness: &mut PtyHarness,
    row: u16,
    column: u16,
    cells: u16,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        let inverse = harness.screen().inverse_columns(row);
        let highlighted = inverse
            .iter()
            .filter(|&&cell| (column..column + cells).contains(&cell))
            .count();
        if highlighted >= cells as usize {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "{what}; inverse columns in row {row}: {inverse:?}\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Covers: dragging over history text must update the selection highlight
// before the button is released, not only on mouse up.
// Owner: interactive UX (PTY).
fn assert_drag_updates_highlight_before_release(harness: &mut PtyHarness) -> Result<()> {
    let (row, column) = screen_cell(harness, "fixture response: drag select target")?;
    // SGR mouse coordinates are 1-based.
    let press = (column + 1, row + 1);
    harness.mouse(MouseButton::Left, press.0, press.1, true)?;
    harness.mouse_drag(press.0 + DRAG_WIDTH, press.1)?;

    wait_for_row_highlight(
        harness,
        row,
        column,
        DRAG_WIDTH,
        "selection highlight did not follow the drag before release",
    )?;

    harness.mouse(MouseButton::Left, press.0 + DRAG_WIDTH, press.1, false)?;
    Ok(())
}

// Covers: drag selection outside the history area (composer, statusline) must
// highlight during the drag and copy the selected screen text on release.
// Owner: interactive UX (PTY).
fn assert_screen_drag_copies_composer_text(harness: &mut PtyHarness) -> Result<()> {
    let (row, column) = screen_cell(harness, "grab this text")?;
    let selected_cells: u16 = 4;
    let press = (column + 1, row + 1);
    harness.mouse(MouseButton::Left, press.0, press.1, true)?;
    harness.mouse_drag(press.0 + selected_cells - 1, press.1)?;

    wait_for_row_highlight(
        harness,
        row,
        column,
        selected_cells,
        "screen selection highlight did not appear during drag",
    )?;

    harness.mouse(
        MouseButton::Left,
        press.0 + selected_cells - 1,
        press.1,
        false,
    )?;

    // "grab" was selected, so the copy notice reports exactly 4 chars.
    harness.wait_for_text("4 chars", WaitTimeout::secs(5, "screen copy notice"))?;
    Ok(())
}

pub(super) const SCREEN_TEXT_SELECTION_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("type_composer_text"),
    Step::TypeText("grab this text"),
    Step::WaitText {
        text: "grab this text",
        timeout: STREAM,
    },
    Step::Phase("drag_and_copy_screen_text"),
    Step::Custom(assert_screen_drag_copies_composer_text),
    Step::Key(crate::keys::Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) const TEXT_SELECTION_DRAG_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("render_response"),
    Step::SubmitText("drag select target"),
    Step::WaitText {
        text: "fixture response: drag select target",
        timeout: STREAM,
    },
    Step::Phase("drag_and_check_highlight"),
    Step::Custom(assert_drag_updates_highlight_before_release),
    Step::ExitCommand,
];

#[cfg(test)]
mod tests {
    use super::display_column_at_byte;
    use pretty_assertions::assert_eq;

    #[test]
    fn display_column_counts_multibyte_and_wide_prefix() {
        // "α" is 2 UTF-8 bytes and 1 display column; "宽" is 3 bytes and 2 columns.
        let line = "α宽target";
        let byte_offset = line.find("target").expect("needle present");
        assert_eq!(byte_offset, 5);
        assert_eq!(display_column_at_byte(line, byte_offset), 3);
        assert_eq!(display_column_at_byte("ascii target", 6), 6);
    }
}
