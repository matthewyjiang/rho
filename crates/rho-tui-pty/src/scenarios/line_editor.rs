//! Single-line overlays must show edits at the insertion point, not a fixed prefix.

use std::time::Duration;

use anyhow::{ensure, Result};

use crate::{
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
    PtyHarness,
};

use super::{SETTLE, STARTUP};

const STEPS: &[Step] = &[
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/login"),
    Step::WaitText {
        text: "Select provider to login",
        timeout: SETTLE,
    },
    Step::TypeText("Chat Completions"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit provider name",
        timeout: SETTLE,
    },
    Step::TypeText("viewport-test"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit base URL",
        timeout: SETTLE,
    },
    Step::Phase("edit_beyond_right_edge"),
    Step::Key(Key::End),
    Step::TypeText("/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaTAIL"),
    Step::WaitText {
        text: "TAIL",
        timeout: SETTLE,
    },
    Step::Key(Key::Backspace),
    Step::WaitTextGone {
        text: "TAIL",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "TAI",
        timeout: SETTLE,
    },
    Step::Phase("navigate_across_viewport"),
    Step::Key(Key::Home),
    Step::WaitText {
        text: "http://127.0.0.1:8000/v1",
        timeout: SETTLE,
    },
    Step::Key(Key::End),
    Step::WaitText {
        text: "TAI",
        timeout: SETTLE,
    },
    Step::Key(Key::Left),
    Step::TypeText("X"),
    Step::WaitText {
        text: "TAXI",
        timeout: SETTLE,
    },
    Step::Phase("unicode_and_resize"),
    Step::Key(Key::End),
    Step::Paste("界界界界界界界界界界界界界界界界界界界界e\u{301}END"),
    Step::WaitText {
        text: "e\u{301}END",
        timeout: SETTLE,
    },
    Step::Resize { rows: 24, cols: 20 },
    Step::WaitText {
        text: "e\u{301}END",
        timeout: SETTLE,
    },
    Step::Key(Key::Backspace),
    Step::WaitTextGone {
        text: "e\u{301}END",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "e\u{301}EN",
        timeout: SETTLE,
    },
    // Cancel without persisting the deliberately invalid endpoint.
    Step::Key(Key::Esc),
    Step::Resize { rows: 24, cols: 40 },
    Step::Phase("masked_caret"),
    Step::Custom(check_masked_editor),
    Step::ExitCommand,
];

// Covers: secret glyph widths must not move the caret away from the mask, and
// scrolling must not expose the underlying key. Owner: interactive TUI.
fn check_masked_editor(harness: &mut PtyHarness) -> Result<()> {
    harness.submit_text("/login openai")?;
    harness.wait_for_text("enter OpenAI API key", SETTLE)?;
    harness.paste("界界")?;
    harness.wait_for_text("••", SETTLE)?;
    harness.wait_for_quiet(Duration::from_millis(150), SETTLE)?;
    let (row, column) = harness.screen().cursor();
    ensure!(
        (
            harness.screen().rows_text()[usize::from(row)].trim_end(),
            column
        ) == ("••", 2),
        "masked caret does not match the two displayed bullets: row={row}, column={column}\n{}",
        harness.screen().debug_dump()
    );
    ensure!(!harness.screen().contains_text("界"), "secret leaked");

    harness.paste(&"s".repeat(50))?;
    let caret_column = harness.screen().cols() - 1;
    let visible_mask = "•".repeat(usize::from(caret_column));
    harness.wait_for_text(&visible_mask, SETTLE)?;
    harness.wait_for_quiet(Duration::from_millis(150), SETTLE)?;
    let (row, column) = harness.screen().cursor();
    ensure!(
        (
            harness.screen().rows_text()[usize::from(row)].trim_end(),
            column
        ) == (visible_mask.as_str(), caret_column),
        "masked tail must leave a cell for the caret"
    );
    harness.inject_key(&Key::Esc)
}

pub(super) const LINE_EDITOR_SCENARIO: Scenario = Scenario::new(
    "line_editor_viewport",
    "Keep single-line edits visible across navigation, Unicode, and resize",
    PtySize { rows: 24, cols: 40 },
    STEPS,
    /*smoke*/ true,
);
