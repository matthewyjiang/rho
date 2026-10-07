//! Checklist status and overflow through a native tool call, even in Plan mode.

use anyhow::{ensure, Result};

use crate::{
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP, STREAM};

pub(super) const TODO_CARD_SCENARIO: Scenario = Scenario::new(
    "todo_card",
    "Render native checklist statuses and overflow in Plan mode",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture todo"),
        Step::WaitText {
            text: "todo checklist complete",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "Worked for",
            timeout: STREAM,
        },
        // The generic row budget hides the tail, but expansion must reveal
        // actual checklist items rather than an adapter-level overflow count.
        Step::Custom(assert_tail_hidden),
        Step::Key(Key::Ctrl('o')),
        Step::WaitText {
            text: "○ remaining step 12",
            timeout: STREAM,
        },
        Step::AssertText("▸ implement checklist"),
        Step::AssertText("✓ inspect requirements"),
        Step::AssertText("○ remaining step 4"),
        Step::Key(Key::Ctrl('o')),
        Step::WaitTextGone {
            text: "remaining step 12",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_args(&["--permission-mode", "plan"]);

fn assert_tail_hidden(harness: &mut PtyHarness) -> Result<()> {
    ensure!(
        !harness.screen().contains_text("remaining step 12"),
        "collapsed checklist exposed its tail:\n{}",
        harness.screen().contents()
    );
    Ok(())
}
