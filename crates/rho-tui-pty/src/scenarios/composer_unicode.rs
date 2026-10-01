use anyhow::{Context, Result};

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{line_editor::wait_for_tail_caret, SETTLE, STARTUP};

// Covers: sequence-width mismatches must not clip text or add composer rows.
// Owner: interactive UX (narrow composer wrapping and caret placement).
fn check_unicode_composer(harness: &mut PtyHarness) -> Result<()> {
    for (input, extra_rows) in [("❤️❤️❤️❤️❤️ab", 1), ("👨‍👩‍👧‍👦ab", 0)]
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
        // Emoji widths in the screen model differ from Rho's, so the caret
        // target comes from the measured content column, not the row text.
        let end = (row, content_column + 4);
        harness.wait_for_cursor(end, SETTLE).with_context(|| {
            format!("input {input:?}: caret is not after the four-column trailing row")
        })?;
        harness.inject_key(&Key::Home)?;
        // A quiet PTY can mean the input has not been processed yet. Wait for
        // Home's observable caret movement, not an idle interval under CI load.
        harness.wait_for_cursor((end.0 - extra_rows, content_column), SETTLE)?;
        harness.inject_key(&Key::Ctrl('c'))?;
        harness.wait_for_text_gone("ab", SETTLE)?;
    }
    Ok(())
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
