//! History and composer text-selection pointer behavior.

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

// Covers: click and drag edit the free-text composer, double-click selection
// survives release, and a clipped composer keeps text stable under the pointer.
// Owner: interactive UX (PTY).
fn assert_composer_click_and_replace_selection(harness: &mut PtyHarness) -> Result<()> {
    let (row, column) = screen_cell(harness, "grab this text")?;
    // Click just after "grab " so the caret sits before "this".
    let click_col = column + "grab ".chars().count() as u16;
    let press = (click_col + 1, row + 1);
    harness.mouse(MouseButton::Left, press.0, press.1, true)?;
    harness.mouse(MouseButton::Left, press.0, press.1, false)?;
    harness.type_text("X")?;
    harness.wait_for_text(
        "grab Xthis text",
        WaitTimeout::secs(5, "click-to-place insert"),
    )?;

    // Select "this" (caret before 't' through caret after 's') and replace it.
    let (row, column) = screen_cell(harness, "grab Xthis text")?;
    let this_col = column + "grab X".chars().count() as u16;
    let after_this_col = this_col + "this".chars().count() as u16;
    let press = (this_col + 1, row + 1);
    let release = (after_this_col + 1, row + 1);
    harness.mouse(MouseButton::Left, press.0, press.1, true)?;
    harness.mouse_drag(release.0, release.1)?;
    wait_for_row_highlight(
        harness,
        row,
        this_col,
        "this".chars().count() as u16,
        "composer selection highlight did not appear during drag",
    )?;
    harness.mouse(MouseButton::Left, release.0, release.1, false)?;
    harness.type_text("that")?;
    harness.wait_for_text(
        "grab Xthat text",
        WaitTimeout::secs(5, "selection replace typing"),
    )?;

    // Complete both press/release pairs before typing over the selected word.
    let (row, column) = screen_cell(harness, "grab Xthat text")?;
    let word_col = column + "grab X".chars().count() as u16;
    let click = (word_col + 1, row + 1);
    for _ in 0..2 {
        harness.mouse(MouseButton::Left, click.0, click.1, true)?;
        harness.mouse(MouseButton::Left, click.0, click.1, false)?;
    }
    harness.type_text("WORD")?;
    harness.wait_for_text(
        "grab WORD text",
        WaitTimeout::secs(5, "double-click selection replace"),
    )?;

    // With only two composer rows visible, clicking the top row must not move
    // another source row under the pointer before mouse release.
    harness.inject_key(&crate::keys::Key::Ctrl('c'))?;
    harness.resize(6, 20)?;
    let wrapped = "111111111111111 222222222222222 333333333333333 444444444444444";
    harness.type_text(wrapped)?;
    harness.wait_for_text("444444", WaitTimeout::secs(5, "wrapped composer"))?;
    let target = "333333333333333";
    let (row, column) = screen_cell(harness, target)?;
    let click = (column + target.chars().count() as u16 + 1, row + 1);
    harness.mouse(MouseButton::Left, click.0, click.1, true)?;
    harness.mouse(MouseButton::Left, click.0, click.1, false)?;
    harness.type_text("X")?;
    harness.wait_for_text(
        "333333333333333X",
        WaitTimeout::secs(5, "clipped composer click"),
    )?;

    // Dragging composer text must not fire the screen-copy notice path.
    if harness.screen().contains_text(" chars ") || harness.screen().contains_text("chars copied") {
        anyhow::bail!(
            "composer drag unexpectedly copied text:\n{}",
            harness.screen().debug_dump()
        );
    }
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
    Step::Phase("click_place_and_replace_selection"),
    Step::Custom(assert_composer_click_and_replace_selection),
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
