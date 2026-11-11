//! `/doctor` overlay dashboard scenario.

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{mcp::setup_mcp, SETTLE, STARTUP};

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

// Covers: the MCP row reports the session's servers, a failed one included.
// `daemon_doctor_mcp` expects the same row from a daemon session.
// Owner: interactive TUI
const DOCTOR_MCP_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    // The doctor row is a snapshot: wait until the broken server has failed.
    Step::Phase("servers_settle"),
    Step::SubmitText("/mcp"),
    Step::WaitText {
        text: "1 problem",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "1 problem",
        timeout: SETTLE,
    },
    Step::Phase("open_doctor"),
    Step::SubmitText("/doctor"),
    Step::WaitText {
        text: "Authentication",
        timeout: SETTLE,
    },
    // The Extensions section sits below the fold.
    Step::Key(Key::End),
    Step::WaitText {
        text: "1 server problem, 0 connected, 0 tools; run /mcp for details",
        timeout: SETTLE,
    },
    Step::AssertText("degraded"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Extensions",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const DOCTOR_MCP_SIZE: PtySize = PtySize {
    rows: 50,
    cols: 120,
};

pub(super) const DOCTOR_MCP_SCENARIO: Scenario = Scenario::new(
    "doctor_mcp",
    "Report the session's MCP servers in /doctor",
    DOCTOR_MCP_SIZE,
    DOCTOR_MCP_STEPS,
    /* smoke */ false,
)
.with_setup(setup_mcp);

fn assert_doctor_overlay_is_single_pane(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_hidden_cursor(SETTLE)?;
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
    harness.wait_for_visible_cursor(SETTLE)?;
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
