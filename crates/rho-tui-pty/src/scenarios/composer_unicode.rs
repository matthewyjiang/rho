use anyhow::{Context, Result};

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
    screen::ScreenModel,
};

use super::{line_editor::wait_for_tail_caret, SETTLE, STARTUP};

// Covers: sequence-width mismatches must not clip text or add composer rows.
// Owner: interactive UX (narrow composer wrapping and caret placement).
fn check_unicode_composer(harness: &mut PtyHarness) -> Result<()> {
    // The intact check is skipped for ZWJ sequences: vt100 stores each person
    // of the family emoji in its own cell, so no cell can hold the whole glyph.
    for (input, intact, extra_rows) in [("❤️❤️❤️❤️❤️ab", Some(("❤️", 5)), 1), ("👨‍👩‍👧‍👦ab", None, 0)]
    {
        harness.resize(24, 10)?;
        harness.type_text("z")?;
        // Text can paint before its caret flush, so reading the cursor as soon
        // as "z" appears can see the row-end pending wrap. Wait for the caret.
        let content_column = wait_for_tail_caret(harness, "z")?.1 - 1;
        harness.inject_key(&Key::Backspace)?;
        harness.type_text(input)?;
        harness.wait_for_text("ab", SETTLE)?;
        let row = harness
            .screen()
            .rows_text()
            .iter()
            .position(|line| line.contains("ab"))
            .context("trailing text was not rendered on one row")? as u16;
        // The caret target comes from the measured content column, not the
        // row text, so it does not depend on how the screen model sizes emoji.
        let end = (row, content_column + 4);
        harness.wait_for_cursor(end, SETTLE).with_context(|| {
            format!("input {input:?}: caret is not after the four-column trailing row")
        })?;
        if let Some((cluster, expected)) = intact {
            let drawn = wide_cells(harness.screen(), cluster);
            anyhow::ensure!(
                drawn == expected,
                "input {input:?}: {drawn} of {expected} {cluster:?} clusters drawn wide\n{}",
                harness.screen().debug_dump()
            );
        }
        harness.inject_key(&Key::Home)?;
        // A quiet PTY can mean the input has not been processed yet. Wait for
        // Home's observable caret movement, not an idle interval under CI load.
        harness.wait_for_cursor((end.0 - extra_rows, content_column), SETTLE)?;
        harness.inject_key(&Key::Ctrl('c'))?;
        harness.wait_for_text_gone("ab", SETTLE)?;
    }
    Ok(())
}

/// Counts screen cells that still hold `cluster` as one wide glyph, so a
/// write over an emoji's right half (which erases it) is caught.
fn wide_cells(screen: &ScreenModel, cluster: &str) -> usize {
    (0..screen.rows())
        .flat_map(|row| (0..screen.cols()).map(move |col| (row, col)))
        .filter_map(|(row, col)| screen.cell(row, col))
        .filter(|cell| cell.wide && cell.contents == cluster)
        .count()
}

pub(super) const COMPOSER_UNICODE_SCENARIO: Scenario = Scenario::new(
    "composer_unicode",
    "Keep emoji clusters intact and trailing text visible in a narrow composer",
    PtySize { rows: 24, cols: 80 },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(check_unicode_composer),
        Step::Resize { rows: 24, cols: 80 },
        Step::ExitCommand,
    ],
    true,
);
