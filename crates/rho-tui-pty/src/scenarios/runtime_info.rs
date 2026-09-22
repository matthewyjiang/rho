//! `/info` opens a single-pane overlay, copies from it, and keeps fields readable
//! when the pane narrows.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use unicode_width::UnicodeWidthChar;

use crate::{
    harness::PtyHarness,
    keys::{Key, MouseButton},
    scenario::Step,
};

use super::{SETTLE, STARTUP};

pub(super) const RUNTIME_INFO_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "rho",
        timeout: STARTUP,
    },
    Step::Phase("open_info"),
    Step::SubmitText("/info"),
    Step::WaitText {
        text: "Model",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Session usage",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Workspace",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Permissions",
        timeout: SETTLE,
    },
    Step::Custom(assert_info_overlay_is_single_pane),
    Step::Custom(copy_and_select_info),
    Step::Resize { rows: 44, cols: 30 },
    // Poll for the stacked layout instead of waiting for output to go quiet:
    // a loaded runner can leave the screen blank between the clear and the
    // redraw long enough for a quiet window to pass.
    Step::Custom(wait_until_runtime_info_stacked),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Session usage",
        timeout: SETTLE,
    },
    Step::Custom(assert_info_overlay_dismissed),
    Step::ExitCommand,
];

fn assert_info_overlay_is_single_pane(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_hidden_cursor(SETTLE)?;
    let screen = harness.screen().contents();
    if !screen.contains(" Info ") {
        bail!("info overlay title missing:\n{screen}");
    }
    if screen.contains("Search") || screen.contains("DETAILS") {
        bail!("info overlay used picker chrome:\n{screen}");
    }
    if !harness.screen().hide_cursor() {
        bail!(
            "info overlay must hide the terminal caret, cursor at {:?}:\n{screen}",
            harness.screen().cursor()
        );
    }
    Ok(())
}

const CLIPBOARD: &[u8] = b"\x1b]52;c;";
const DRAG_WIDTH: u16 = 8;

fn copy_and_select_info(harness: &mut PtyHarness) -> Result<()> {
    let before = harness.raw_sequence_occurrences(CLIPBOARD);
    harness.inject_key(&Key::Char('c'))?;
    harness.wait_for_raw_sequence_occurrences(CLIPBOARD, before + 1, SETTLE)?;
    if !harness.screen().contents().contains(" Info ") {
        bail!(
            "c closed the info overlay:\n{}",
            harness.screen().contents()
        );
    }

    let (row, column) = screen_cell(harness, "Permissions")?;
    let press = (column + 1, row + 1);
    harness.mouse(MouseButton::Left, press.0, press.1, true)?;
    harness.mouse_drag(press.0 + DRAG_WIDTH, press.1)?;
    wait_for_row_highlight(harness, row, column, DRAG_WIDTH)?;
    harness.mouse(MouseButton::Left, press.0 + DRAG_WIDTH, press.1, false)?;
    harness.wait_for_raw_sequence_occurrences(CLIPBOARD, before + 2, SETTLE)?;
    if !harness.screen().contents().contains(" Info ") {
        bail!(
            "selecting text closed the info overlay:\n{}",
            harness.screen().contents()
        );
    }
    Ok(())
}

fn screen_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate() {
        if let Some(byte_offset) = line.find(needle) {
            let column = line
                .get(..byte_offset)
                .unwrap_or("")
                .chars()
                .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
                .sum::<usize>() as u16;
            return Ok((row as u16, column));
        }
    }
    bail!("'{needle}' not found:\n{}", harness.screen().debug_dump())
}

fn wait_for_row_highlight(
    harness: &mut PtyHarness,
    row: u16,
    column: u16,
    cells: u16,
) -> Result<()> {
    let deadline = Instant::now() + SETTLE.duration;
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
            bail!(
                "info selection highlight did not follow the drag; inverse columns in row {row}: {inverse:?}\n{}",
                harness.screen().debug_dump()
            );
        }
    }
}

fn assert_info_overlay_dismissed(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_visible_cursor(SETTLE)?;
    // The copy notice covers the header brand until it expires and the loop
    // redraws. Wait for that paint instead of reading the covered frame once.
    harness.wait_for_text("rho", SETTLE)?;
    let screen = harness.screen().contents();
    if screen.contains(" Info ") {
        bail!("info overlay still visible after Esc:\n{screen}");
    }
    if screen.contains("Session usage") {
        bail!("runtime info stayed in the transcript after Esc:\n{screen}");
    }
    if harness.screen().hide_cursor() {
        bail!("composer caret still hidden after dismissing info:\n{screen}");
    }
    Ok(())
}

fn wait_until_runtime_info_stacked(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + SETTLE.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        if runtime_info_stacked(harness) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "runtime info did not stack the Permissions field after resize:\n{}",
                harness.screen().debug_dump()
            );
        }
    }
}

fn runtime_info_stacked(harness: &PtyHarness) -> bool {
    let rows = harness.screen().rows_text();
    rows.iter()
        .position(|row| panel_cell(row) == "Permissions")
        .and_then(|index| rows.get(index + 1))
        .is_some_and(|row| panel_cell(row) == "bypass")
}

/// Drop overlay borders and the scrollbar so a stacked field can be compared
/// as plain text. The track is `│` and the thumb is `█`.
fn panel_cell(row: &str) -> String {
    row.chars()
        .filter(|ch| !matches!(ch, '│' | '█' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '─'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
