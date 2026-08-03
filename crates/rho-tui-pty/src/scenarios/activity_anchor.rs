//! Activity-rail anchoring during a live turn.

use std::time::Duration;

use anyhow::Result;

use crate::{
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

// Covers: while a turn is live, the activity rail stays above the composer.
// Owner: interactive TUI
const SPINNER_ACTIVITY_ANCHOR_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_turn"),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "responding",
        timeout: STREAM,
    },
    Step::Custom(assert_activity_anchored_above_composer),
    Step::Phase("cancel"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

// Covers: after scrolling away from bottom during a live turn, jump-to-bottom
// shares the activity rail row.
// Owner: interactive TUI
const SPINNER_ACTIVITY_JUMP_RAIL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("seed_history"),
    Step::SubmitText("fixture bulk one"),
    Step::WaitText {
        text: "fixture bulk one line 180",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Phase("start_turn"),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "responding",
        timeout: STREAM,
    },
    Step::Phase("scroll_away"),
    Step::Key(Key::PageUp),
    Step::Key(Key::PageUp),
    Step::Key(Key::PageUp),
    Step::WaitText {
        text: "bottom",
        timeout: WaitTimeout::secs(5, "jump control while scrolled"),
    },
    Step::Custom(assert_activity_shares_rail_with_jump),
    Step::Phase("cancel"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const SPINNER_ACTIVITY_ANCHOR_SCENARIO: Scenario = Scenario::new(
    "spinner_activity_anchor",
    "Keep the activity rail above the composer during a live turn",
    SIZE,
    SPINNER_ACTIVITY_ANCHOR_STEPS,
    /* smoke */ true,
);

pub(super) const SPINNER_ACTIVITY_JUMP_RAIL_SCENARIO: Scenario = Scenario::new(
    "spinner_activity_jump_rail",
    "Keep jump-to-bottom on the activity rail after scrolling away",
    SIZE,
    SPINNER_ACTIVITY_JUMP_RAIL_STEPS,
    /* smoke */ false,
);

fn composer_row(rows: &[String]) -> Option<usize> {
    rows.iter().position(|row| {
        let trimmed = row.trim_start();
        trimmed == ">" || trimmed.starts_with("> ")
    })
}

fn assert_activity_anchored_above_composer(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let activity_row = rows
        .iter()
        .position(|row| row.contains("responding"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "activity rail label missing while turn is live:\n{}",
                harness.screen().contents()
            )
        })?;
    let composer_row = composer_row(&rows).ok_or_else(|| {
        anyhow::anyhow!(
            "composer prompt missing while turn is live:\n{}",
            harness.screen().contents()
        )
    })?;
    if activity_row >= composer_row {
        anyhow::bail!(
            "activity rail row {activity_row} is not above composer row {composer_row}:\n{}",
            harness.screen().contents()
        );
    }
    if rows[activity_row].contains("partial assistant before cancellation") {
        anyhow::bail!(
            "transcript content occupied the activity rail row:\n{}",
            harness.screen().contents()
        );
    }
    Ok(())
}

fn assert_activity_shares_rail_with_jump(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let activity_row = rows
        .iter()
        .position(|row| row.contains("responding"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "activity rail label missing after scroll:\n{}",
                harness.screen().contents()
            )
        })?;
    let jump_row = rows
        .iter()
        .position(|row| row.contains("bottom") && row.contains('↓'))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "jump-to-bottom control missing after scroll:\n{}",
                harness.screen().contents()
            )
        })?;
    if activity_row != jump_row {
        anyhow::bail!(
            "activity rail row {activity_row} and jump control row {jump_row} diverged:\n{}",
            harness.screen().contents()
        );
    }
    Ok(())
}
