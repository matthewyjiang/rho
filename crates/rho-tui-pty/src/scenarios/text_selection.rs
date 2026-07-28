//! History text-selection pointer behavior.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::{STARTUP, STREAM};
use crate::{harness::WaitTimeout, keys::MouseButton, scenario::Step, PtyHarness};

const DRAG_WIDTH: u16 = 8;

/// Finds the assistant response row and the 0-based screen column where its
/// text starts.
fn response_cell(harness: &PtyHarness) -> Result<(u16, u16)> {
    let needle = "fixture response: drag select target";
    for (row, line) in harness.screen().rows_text().iter().enumerate() {
        if let Some(column) = line.find(needle) {
            return Ok((row as u16, column as u16));
        }
    }
    anyhow::bail!(
        "assistant response row not found:\n{}",
        harness.screen().debug_dump()
    );
}

// Covers: dragging over history text must update the selection highlight
// before the button is released, not only on mouse up.
// Owner: interactive UX (PTY).
fn assert_drag_updates_highlight_before_release(harness: &mut PtyHarness) -> Result<()> {
    let (row, column) = response_cell(harness)?;
    // SGR mouse coordinates are 1-based.
    let press = (column + 1, row + 1);
    harness.mouse(MouseButton::Left, press.0, press.1, true)?;
    harness.mouse_drag(press.0 + DRAG_WIDTH, press.1)?;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        let inverse = harness.screen().inverse_columns(row);
        let highlighted = inverse
            .iter()
            .filter(|&&cell| (column..column + DRAG_WIDTH).contains(&cell))
            .count();
        if highlighted >= DRAG_WIDTH as usize {
            break;
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "selection highlight did not follow the drag before release; \
                 inverse columns in row {row}: {inverse:?}\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    harness.mouse(MouseButton::Left, press.0 + DRAG_WIDTH, press.1, false)?;
    Ok(())
}

/// Finds the composer row and 0-based column where `needle` is rendered.
fn screen_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate() {
        if let Some(column) = line.find(needle) {
            return Ok((row as u16, column as u16));
        }
    }
    anyhow::bail!("'{needle}' not found:\n{}", harness.screen().debug_dump());
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

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        let inverse = harness.screen().inverse_columns(row);
        let highlighted = inverse
            .iter()
            .filter(|&&cell| (column..column + selected_cells).contains(&cell))
            .count();
        if highlighted >= selected_cells as usize {
            break;
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "screen selection highlight did not appear during drag; \
                 inverse columns in row {row}: {inverse:?}\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }

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
