//! An unfocused terminal gets an OSC 9 notification when an approval opens and
//! again when the turn finishes.

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::{PtyHarness, WaitTimeout},
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP};

const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const FOCUS_LOST: &[u8] = b"\x1b[O";
const APPROVAL: &[u8] = b"\x1b]9;rho: waiting for approval\x07";
const FINISHED: &[u8] = b"\x1b]9;rho: turn finished\x07";

// Covers: focus reporting gates notifications, an approval notifies at once,
// and the finished turn notifies when the TUI returns to idle.
// Owner: interactive TUI event loop (notifier wiring), not the pure notifier.
pub(super) const TURN_NOTIFICATIONS_SCENARIO: Scenario = Scenario::new(
    "turn_notifications",
    "Notify an unfocused terminal when an approval opens and when the turn finishes",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(lose_focus),
        Step::SubmitText("fixture approval long"),
        Step::Phase("approval_notifies"),
        Step::Custom(wait_for_approval_notification),
        Step::Phase("finished_turn_notifies"),
        Step::Custom(deny_and_wait_for_finished_notification),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
// Ghostty speaks OSC 9; without a known terminal Rho falls back to BEL, which
// the raw stream cannot tell apart from OSC terminators.
.with_env(&[("TERM_PROGRAM", "ghostty")])
.with_setup(setup_supervised);

fn setup_supervised(home: &IsolatedHome) -> Result<()> {
    let config = std::fs::read_to_string(&home.config_path)?;
    // Top-level keys must precede the first table header.
    std::fs::write(
        &home.config_path,
        format!("permission_mode = \"supervised\"\n{config}"),
    )?;
    Ok(())
}

fn lose_focus(harness: &mut PtyHarness) -> Result<()> {
    harness.inject_bytes(FOCUS_LOST)
}

fn wait_for_approval_notification(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("Allow for session", STREAM)?;
    harness.wait_for_raw_sequence_occurrences(APPROVAL, 1, STREAM)
}

fn deny_and_wait_for_finished_notification(harness: &mut PtyHarness) -> Result<()> {
    // Deny is the default choice; Enter resolves it and the turn continues.
    harness.inject_key(&crate::keys::Key::Enter)?;
    harness.wait_for_raw_sequence_occurrences(FINISHED, 1, STREAM)?;
    let approvals = harness.raw_sequence_occurrences(APPROVAL);
    anyhow::ensure!(approvals == 1, "approval notified {approvals} times");
    Ok(())
}
