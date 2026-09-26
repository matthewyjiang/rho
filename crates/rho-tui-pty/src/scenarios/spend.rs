//! `/spend` overlay scenario.

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

// Short enough that the report scrolls, so the panel reserves a scrollbar
// column beside the right-aligned totals.
const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 110,
};

// Covers: a completed turn lands in the isolated usage ledger and /spend reads
// it back without clipping right-aligned totals beside the scrollbar; Tab
// switches ranges in place; Esc closes without a transcript block.
// Owner: interactive TUI
const SPEND_OVERLAY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("record_turn"),
    Step::SubmitText("spend seed"),
    Step::WaitText {
        text: "fixture response: spend seed",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Worked for",
        timeout: STREAM,
    },
    Step::Phase("open_spend"),
    Step::SubmitText("/spend"),
    Step::WaitText {
        text: "Equivalent API cost",
        timeout: SETTLE,
    },
    Step::Custom(assert_spend_shows_recorded_turn),
    Step::Phase("switch_range"),
    // Opens on Today (hourly axis from 00:00); Tab moves to 7 days, whose
    // daily axis starts at a date instead.
    Step::Key(Key::Tab),
    Step::Custom(assert_switched_to_seven_days),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Equivalent API cost",
        timeout: SETTLE,
    },
    Step::Custom(assert_spend_dismissed),
    Step::ExitCommand,
];

pub(super) const SPEND_OVERLAY_SCENARIO: Scenario = Scenario::new(
    "spend_overlay",
    "Record a turn, read it back in /spend, switch ranges, and dismiss",
    SIZE,
    SPEND_OVERLAY_STEPS,
    /* smoke */ false,
);

fn assert_spend_shows_recorded_turn(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_hidden_cursor(SETTLE)?;
    let screen = harness.screen().contents();
    for needle in ["00:00", "Providers", "2 requests"] {
        if !screen.contains(needle) {
            anyhow::bail!("spend overlay missing {needle:?}:\n{screen}");
        }
    }
    // The fixture has no price, so the headline is `$0.00`. A body laid out
    // wider than the scrollbar leaves clips it to `$0.0…` against the track.
    let headline = screen
        .lines()
        .find(|line| line.contains("Equivalent API cost"))
        .unwrap_or_default();
    if !headline.contains("$0.00") || headline.contains('…') {
        anyhow::bail!("spend headline total clipped: {headline:?}\n{screen}");
    }
    if screen.contains("No model requests recorded yet") {
        anyhow::bail!("spend overlay did not read the recorded turn:\n{screen}");
    }
    Ok(())
}

fn assert_spend_dismissed(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_visible_cursor(SETTLE)?;
    let screen = harness.screen().contents();
    if screen.contains("Equivalent API cost") {
        anyhow::bail!("spend overlay still visible after Esc:\n{screen}");
    }
    if !screen.contains("fixture response: spend seed") {
        anyhow::bail!("transcript missing after dismissing spend:\n{screen}");
    }
    Ok(())
}

fn assert_switched_to_seven_days(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text_gone("00:00", SETTLE)?;
    // The recorded turn keeps the report populated; a blank body would lose
    // the Models table along with the hourly axis.
    harness.wait_for_text("Models", SETTLE)
}
