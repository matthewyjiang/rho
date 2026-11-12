//! A daemon turn's approval request opens the TUI's approval prompt, and the
//! person's choice answers it (#1667).

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{daemon_resume::write_daemon_config_in_mode, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

const ENV: &[(&str, &str)] = &[("OPENAI_API_KEY", "sk-test-matrix")];
/// The idle composer's placeholder; a running turn shows `esc abort`.
const IDLE_COMPOSER: &str = "Type a message";
/// The approval prompt's title for the fixture's `write` call.
const PROMPT_TITLE: &str = "write wants to write";
/// The fixture's answer once its `write` call returns, allowed or denied.
const ANSWER: &str = "tool lifecycle complete with one result";
/// What the call returns once the person denied it.
const DENIED: &str = "denied by user";

// Covers: in supervised mode, a daemon turn's write opens the TUI's approval
// prompt; choosing Allow once answers the daemon, whose turn then completes
// with the write done.
// Owner: interactive TUI as a daemon client
pub(super) const DAEMON_APPROVAL_SCENARIO: Scenario = Scenario::new(
    "daemon_approval",
    "Approve a daemon turn's write from the TUI's approval prompt",
    DEFAULT_SIZE,
    DAEMON_APPROVAL_STEPS,
    /*smoke*/ false,
)
.with_setup(write_supervised_daemon_config)
.with_env(ENV);

// Covers: Enter on the approval prompt's default denies a daemon turn's
// write, and the turn completes with the call refused.
// Owner: interactive TUI as a daemon client
pub(super) const DAEMON_APPROVAL_DENY_SCENARIO: Scenario = Scenario::new(
    "daemon_approval_deny",
    "Deny a daemon turn's write from the TUI's approval prompt",
    DEFAULT_SIZE,
    DAEMON_APPROVAL_DENY_STEPS,
    /*smoke*/ false,
)
.with_setup(write_supervised_daemon_config)
.with_env(ENV);

const DAEMON_APPROVAL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("prompt"),
    Step::SubmitText("fixture tool"),
    Step::WaitText {
        text: PROMPT_TITLE,
        timeout: STREAM,
    },
    Step::WaitText {
        text: "→ Deny",
        timeout: SETTLE,
    },
    Step::Phase("approve"),
    // Choices stop at the first, so a press too many still lands on it.
    Step::Key(Key::Up),
    Step::Key(Key::Up),
    Step::WaitText {
        text: "→ Allow once",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: ANSWER,
        timeout: STREAM,
    },
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    Step::Custom(assert_not_denied),
    Step::ExitCommand,
];

const DAEMON_APPROVAL_DENY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("prompt"),
    Step::SubmitText("fixture tool"),
    Step::WaitText {
        text: PROMPT_TITLE,
        timeout: STREAM,
    },
    Step::WaitText {
        text: "→ Deny",
        timeout: SETTLE,
    },
    Step::Phase("deny"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: DENIED,
        timeout: STREAM,
    },
    Step::WaitText {
        text: ANSWER,
        timeout: STREAM,
    },
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

fn write_supervised_daemon_config(home: &IsolatedHome) -> Result<()> {
    write_daemon_config_in_mode(home, "supervised")
}

/// The allowed write ran: nothing on screen says the call was refused.
fn assert_not_denied(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    anyhow::ensure!(
        !screen.contains(DENIED),
        "the approved write was refused:\n{screen}"
    );
    Ok(())
}
