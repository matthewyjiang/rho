//! Inline choice options: pointer focus and double-click confirmation.

use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    keys::{Key, MouseButton},
    scenario::{Scenario, Step},
    PtyHarness,
};

/// 1-based SGR cell of the lowest on-screen occurrence of `needle`. The inline
/// choice sits in the composer, below any transcript text that could echo it.
fn click_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate().rev() {
        if let Some(offset) = line.find(needle) {
            let column = UnicodeWidthStr::width(&line[..offset]);
            return Ok((column as u16 + 1, row as u16 + 1));
        }
    }
    anyhow::bail!("'{needle}' not found:\n{}", harness.screen().debug_dump());
}

fn click(harness: &mut PtyHarness, (column, row): (u16, u16)) -> Result<()> {
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.mouse(MouseButton::Left, column, row, false)
}

/// Two clicks on one cell. Moving away first ends any earlier click sequence,
/// so a single click just before cannot pair with the first of these.
fn double_click(harness: &mut PtyHarness, cell: (u16, u16)) -> Result<()> {
    harness.mouse_move(1, 1)?;
    click(harness, cell)?;
    click(harness, cell)
}

// Covers: one click moves inline choice focus without resolving the
// destructive confirmation; a double click on another option (here its detail
// row) confirms that option like Enter.
// Owner: interactive UX (PTY).
fn click_cancel_then_double_click_delete(harness: &mut PtyHarness) -> Result<()> {
    let cancel = click_cell(harness, "Cancel")?;
    click(harness, cancel)?;
    // The focus repaint follows the click, so the choice state is settled.
    harness.wait_for_text("→ c  Cancel", SETTLE)?;
    let screen = harness.screen();
    if !screen.contains_text("Delete session") || screen.contains_text("deleted session") {
        anyhow::bail!(
            "a single click resolved the inline choice:\n{}",
            screen.debug_dump()
        );
    }
    let delete_detail = click_cell(harness, "Permanently remove this saved session")?;
    double_click(harness, delete_detail)?;
    harness.wait_for_text("deleted session", SETTLE)
}

const INLINE_CHOICE_CLICK_STEPS: &[Step] = &[
    Step::Phase("create_saved_session"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("inline choice click target"),
    Step::WaitText {
        text: "fixture response: inline choice click target",
        timeout: STREAM,
    },
    Step::Phase("start_fresh_session"),
    Step::Key(Key::Ctrl('r')),
    Step::WaitText {
        text: "conversation reset",
        timeout: SETTLE,
    },
    Step::Phase("open_delete_choice"),
    Step::SubmitText("/resume"),
    Step::WaitText {
        text: "Resume session",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "inline choice click target",
        timeout: SETTLE,
    },
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "Permanently remove this saved session",
        timeout: SETTLE,
    },
    Step::Phase("click_choice"),
    Step::Custom(click_cancel_then_double_click_delete),
    Step::ExitCommand,
];

pub(super) const INLINE_CHOICE_CLICK_SCENARIO: Scenario = Scenario::new(
    "inline_choice_click",
    "Focus an inline choice option with a click and confirm another with a double click",
    DEFAULT_SIZE,
    INLINE_CHOICE_CLICK_STEPS,
    /*smoke*/ false,
);
