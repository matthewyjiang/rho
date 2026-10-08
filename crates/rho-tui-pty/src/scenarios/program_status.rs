//! OSC 7501 program status follows the session: idle at startup, working
//! while a turn runs, blocked on an approval, done once the turn settles, and
//! cleared on exit.

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::{PtyHarness, WaitTimeout},
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP};

const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const IDLE: &[u8] = b"\x1b]7501;state=idle:app=rho\x1b\\";
const WORKING: &[u8] = b"\x1b]7501;state=working:app=rho\x1b\\";
/// `msg` is base64 of "waiting for approval".
const BLOCKED: &[u8] =
    b"\x1b]7501;state=blocked:app=rho:kind=permission:msg=d2FpdGluZyBmb3IgYXBwcm92YWw=\x1b\\";
const DONE: &[u8] = b"\x1b]7501;state=done:app=rho\x1b\\";
const CLEAR: &[u8] = b"\x1b]7501;state=clear\x1b\\";

// Covers: the TUI reports each lifecycle transition once, in order, and
// removes its record on exit.
// Owner: interactive TUI event loop (program status wiring), not the encoder.
pub(super) const PROGRAM_STATUS_SCENARIO: Scenario = Scenario::new(
    "program_status",
    "Report idle, working, blocked, done, and clear over OSC 7501",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("startup_reports_idle"),
        Step::Custom(wait_for_idle),
        Step::SubmitText("fixture approval long"),
        Step::Phase("approval_reports_blocked"),
        Step::Custom(wait_for_blocked),
        Step::Phase("settled_turn_reports_done"),
        Step::Custom(deny_and_wait_for_done),
        Step::ExitCommand,
        Step::Custom(assert_cleared_once),
    ],
    /*smoke*/ false,
)
// The harness does not answer the OSC 7501 support query; force reporting on.
// Reply detection is unit-tested with the probe parser.
.with_env(&[("RHO_PROGRAM_STATUS", "1")])
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

fn wait_for_idle(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_raw_sequence_occurrences(IDLE, 1, STARTUP)
}

fn wait_for_blocked(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("Allow for session", STREAM)?;
    harness.wait_for_raw_sequence_occurrences(BLOCKED, 1, STREAM)?;
    let working = harness.raw_sequence_occurrences(WORKING);
    anyhow::ensure!(
        working == 1,
        "working reported {working} times before the approval"
    );
    Ok(())
}

fn deny_and_wait_for_done(harness: &mut PtyHarness) -> Result<()> {
    // Deny is the default choice; Enter resolves it and the turn continues.
    harness.inject_key(&crate::keys::Key::Enter)?;
    harness.wait_for_raw_sequence_occurrences(DONE, 1, STREAM)?;
    let working = harness.raw_sequence_occurrences(WORKING);
    anyhow::ensure!(
        working == 2,
        "working reported {working} times; expected once per side of the approval"
    );
    Ok(())
}

fn assert_cleared_once(harness: &mut PtyHarness) -> Result<()> {
    let counts =
        [IDLE, BLOCKED, DONE, CLEAR].map(|sequence| harness.raw_sequence_occurrences(sequence));
    anyhow::ensure!(
        counts == [1; 4],
        "idle, blocked, done, clear counts: {counts:?}"
    );
    Ok(())
}
