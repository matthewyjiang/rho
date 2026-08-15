//! `/limits` overlay dashboard scenario.

use std::time::Duration;

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

// Covers: /limits opens a single-pane overlay and Esc returns to the session
// without dumping a transcript block.
// Owner: interactive TUI
const LIMITS_OVERLAY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_limits"),
    Step::SubmitText("/limits"),
    Step::WaitText {
        text: "Usage limits",
        timeout: SETTLE,
    },
    Step::Custom(assert_limits_overlay_is_single_pane),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Custom(assert_limits_overlay_dismissed),
    Step::ExitCommand,
];

pub(super) const LIMITS_OVERLAY_SCENARIO: Scenario = Scenario::new(
    "limits_overlay",
    "Open the usage limits overlay and dismiss it cleanly",
    SIZE,
    LIMITS_OVERLAY_STEPS,
    /* smoke */ false,
);

fn assert_limits_overlay_is_single_pane(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("Usage limits") {
        anyhow::bail!("limits overlay title missing:\n{screen}");
    }
    if screen.contains("Search") {
        anyhow::bail!("limits overlay used picker search chrome:\n{screen}");
    }
    Ok(())
}

fn assert_limits_overlay_dismissed(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("Usage limits") {
        anyhow::bail!("limits overlay still visible after Esc:\n{screen}");
    }
    if !screen.contains("gpt-5.5") {
        anyhow::bail!("session chrome missing after dismissing limits:\n{screen}");
    }
    Ok(())
}
