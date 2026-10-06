//! Single-line overlays must show edits at the insertion point, not a fixed prefix.

use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use unicode_width::UnicodeWidthStr;

use crate::{
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
    PtyHarness, WaitTimeout,
};

use super::{first_run::FIRST_RUN_SIGNIN_ENV, SETTLE, STARTUP};

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
    Step::Custom(|harness| check_navigation_window(harness, "TAI")),
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
    Step::Custom(|harness| wait_for_tail_caret(harness, "e\u{301}END").map(|_| ())),
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

// Observe the value row before waiting for the cursor flush at its displayed end.
pub(super) fn wait_for_tail_caret(harness: &mut PtyHarness, tail: &str) -> Result<(u16, u16)> {
    harness.wait_for_text(tail, SETTLE)?;
    // Re-derive the tail position on every frame: right after a resize the
    // first frame showing the tail can still be laid out for the old size, so
    // a position pinned from it never matches the settled caret.
    let deadline = Instant::now() + SETTLE.duration;
    loop {
        let position = tail_end(harness, tail)?;
        let screen = harness.screen();
        if !screen.hide_cursor() && screen.cursor() == position {
            return Ok(position);
        }
        if Instant::now() >= deadline {
            // Report through the harness so failure artifacts are written.
            harness.wait_for_cursor(position, WaitTimeout::millis(0, "settled tail caret"))?;
            return Ok(position);
        }
        harness.poll(Duration::from_millis(25));
    }
}

/// Row and display column just past `tail` on the current frame.
fn tail_end(harness: &PtyHarness, tail: &str) -> Result<(u16, u16)> {
    harness
        .screen()
        .rows_text()
        .iter()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(tail)
                .map(|start| (row as u16, line[..start + tail.len()].width() as u16))
        })
        .context("editor tail was not rendered on one row")
}

// Moving inside the visible window must move the caret, not shift the text.
fn check_navigation_window(harness: &mut PtyHarness, tail: &str) -> Result<()> {
    let (row, column) = wait_for_tail_caret(harness, tail)?;
    let before = harness.screen().rows_text()[usize::from(row)]
        .trim_end()
        .to_owned();
    harness.inject_key(&Key::Left)?;
    harness.wait_for_cursor((row, column - 1), SETTLE)?;
    let after_cursor = harness.screen().cursor();
    let after = harness.screen().rows_text()[usize::from(row)]
        .trim_end()
        .to_owned();
    ensure!(
        (after, after_cursor) == (before, (row, column - 1)),
        "moving left within the viewport shifted the text or pinned the caret\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

// Covers: geometry queries must not re-anchor setup's field at terminal width.
// Owner: interactive TUI; normal login paints at the full width and misses this.
fn check_setup_editor(harness: &mut PtyHarness) -> Result<()> {
    // Setup centres an 88-column body in the scenario's 120-column terminal.
    let content_width = 88;
    let content_left = (harness.screen().cols() - content_width) / 2;
    // Longer than both the painted field and the speculative terminal-wide field.
    let value = format!("SETUP-HEAD-{}-SETUP-TAIL", "a".repeat(128));
    let visible_tail = &value[value.len() - usize::from(content_width - 1)..];
    let expected_row = format!("{}{visible_tail}", " ".repeat(usize::from(content_left)));
    let caret_column = content_left + content_width - 1;
    harness.paste(&value)?;
    harness.wait_for_text(&expected_row, SETTLE)?;
    let (row, column) = wait_for_tail_caret(harness, "SETUP-TAIL")?;
    ensure!(
        column == caret_column,
        "setup tail did not reserve a caret cell in its capped field\n{}",
        harness.screen().debug_dump()
    );
    check_navigation_window(harness, "SETUP-TAIL")?;

    // Stay inside the value: returning all the way to End would legitimately
    // restore the tail window and could hide a mouse-only re-anchoring bug.
    harness.inject_key(&Key::Left)?;
    harness.wait_for_cursor((row, caret_column - 2), SETTLE)?;
    // Hover performs another geometry query; Right proves it and the key ran.
    harness.mouse_move(content_left + content_width / 2 + 1, row + 1)?;
    harness.inject_key(&Key::Right)?;
    harness.wait_for_text(&expected_row, SETTLE)?;
    harness.wait_for_cursor((row, caret_column - 1), SETTLE)?;
    ensure!(
        harness.screen().rows_text()[usize::from(row)] == expected_row,
        "hovering inside setup shifted the value before moving right\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

// Covers: secret glyph widths must not move the caret away from the mask, and
// scrolling must not expose the underlying key. Owner: interactive TUI.
fn check_masked_editor(harness: &mut PtyHarness) -> Result<()> {
    harness.submit_text("/login openai")?;
    harness.wait_for_text("enter OpenAI API key", SETTLE)?;
    harness.paste("界界")?;
    let (row, column) = wait_for_tail_caret(harness, "••")?;
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
    harness.wait_for_cursor((row, caret_column), SETTLE)?;
    let column = harness.screen().cursor().1;
    ensure!(
        (
            harness.screen().rows_text()[usize::from(row)].trim_end(),
            column
        ) == (visible_mask.as_str(), caret_column),
        "masked tail must leave a cell for the caret\n{}",
        harness.screen().debug_dump()
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

pub(super) const SETUP_LINE_EDITOR_SCENARIO: Scenario = Scenario::new(
    "setup_line_editor_viewport",
    "Keep the capped setup field stable through caret navigation and mouse geometry",
    PtySize {
        rows: 24,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "Select provider to login",
            timeout: STARTUP,
        },
        Step::TypeText("Chat Completions"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "edit provider name",
            timeout: SETTLE,
        },
        Step::Phase("capped_setup_viewport"),
        Step::Custom(check_setup_editor),
        // Cancel the wizard without saving the fixture name, then leave setup.
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "Select provider to login",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "Type a message",
            timeout: STARTUP,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_env(FIRST_RUN_SIGNIN_ENV);
