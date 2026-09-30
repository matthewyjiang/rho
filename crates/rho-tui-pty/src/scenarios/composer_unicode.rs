use std::time::Duration;

use anyhow::{ensure, Result};

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

// Covers: sequence-width mismatches must not clip text or add composer rows.
// Owner: interactive UX (narrow composer wrapping and caret placement).
fn check_unicode_composer(harness: &mut PtyHarness) -> Result<()> {
    for (input, extra_rows) in [("❤️❤️❤️❤️❤️ab", 1), ("👨‍👩‍👧‍👦ab", 0)]
    {
        harness.resize(24, 10)?;
        harness.type_text("z")?;
        harness.wait_for_text("z", SETTLE)?;
        let content_column = harness.screen().cursor().1 - 1;
        harness.inject_key(&Key::Backspace)?;
        harness.type_text(input)?;
        harness.wait_for_text("ab", SETTLE)?;
        let end = harness.screen().cursor();
        ensure!(
            end.1 == content_column + 4,
            "input {input:?}: caret is not after the four-column trailing row: {end:?}",
        );
        harness.inject_key(&Key::Home)?;
        harness.wait_for_quiet(Duration::from_millis(150), SETTLE)?;
        let start = harness.screen().cursor();
        ensure!(
            start == (end.0 - extra_rows, content_column),
            "input {input:?}: wrong wrap height: start {start:?}, end {end:?}",
        );
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
