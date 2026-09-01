//! `/doctor` overlay dashboard scenario.

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

// Covers: /doctor opens a single-pane dashboard immediately, hides the
// terminal caret, and Esc returns to the session without dumping a
// transcript block. Probe rows are not asserted: the matrix binary runs real
// probes whose results depend on the host.
// Owner: interactive TUI
const DOCTOR_OVERLAY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_doctor"),
    Step::SubmitText("/doctor"),
    Step::WaitText {
        text: "Authentication",
        timeout: SETTLE,
    },
    Step::Custom(assert_doctor_overlay_is_single_pane),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    // Esc can sit in the input parser before the overlay redraws. WaitQuiet
    // succeeds on a still-open static panel; wait until a section is gone.
    Step::WaitTextGone {
        text: "Authentication",
        timeout: SETTLE,
    },
    Step::Custom(assert_doctor_overlay_dismissed),
    Step::ExitCommand,
];

pub(super) const DOCTOR_OVERLAY_SCENARIO: Scenario = Scenario::new(
    "doctor_overlay",
    "Open the doctor dashboard and dismiss it cleanly",
    SIZE,
    DOCTOR_OVERLAY_STEPS,
    /* smoke */ false,
);

fn assert_doctor_overlay_is_single_pane(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("Doctor") {
        anyhow::bail!("doctor overlay title missing:\n{screen}");
    }
    if screen.contains("Search") || screen.contains("DETAILS") {
        anyhow::bail!("doctor overlay used picker chrome:\n{screen}");
    }
    if !harness.screen().hide_cursor() {
        anyhow::bail!(
            "doctor overlay must hide the terminal caret, cursor at {:?}:\n{screen}",
            harness.screen().cursor()
        );
    }
    Ok(())
}

fn assert_doctor_overlay_dismissed(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("Doctor") {
        anyhow::bail!("doctor overlay still visible after Esc:\n{screen}");
    }
    if !screen.contains("gpt-5.5") {
        anyhow::bail!("session chrome missing after dismissing doctor:\n{screen}");
    }
    if harness.screen().hide_cursor() {
        anyhow::bail!("composer caret still hidden after dismissing doctor:\n{screen}");
    }
    Ok(())
}
