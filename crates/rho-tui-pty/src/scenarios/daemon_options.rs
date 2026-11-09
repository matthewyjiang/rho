//! Daemon settings and command refusals through the real interactive client.

use anyhow::{ensure, Context, Result};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

const ENV: &[(&str, &str)] = &[
    ("RHO_TUI_DAEMON", "1"),
    ("OPENAI_API_KEY", "sk-test-matrix"),
];
const INSTRUCTIONS: &str = "# Project instructions\nKeep this instruction unchanged.\n";

// Covers: /permissions changes the daemon's actual tool policy, not just the
// local statusline. A configure queued during a running turn must leave Esc
// responsive, and become visible only after cancellation releases the turn.
// Owner: interactive TUI as a daemon client
pub(super) const PERMISSION_MODE: Scenario = Scenario::new(
    "daemon_permission_mode",
    "Switch daemon permission policy and configure mid-turn without blocking cancellation",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::SubmitText("/permissions supervised"),
        Step::WaitText {
            text: "Supervised ·",
            timeout: SETTLE,
        },
        Step::SubmitText("fixture tool"),
        Step::WaitText {
            text: "write wants to write",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "→ Deny",
            timeout: SETTLE,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "denied by user",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "tool lifecycle complete with one result",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "Type a message",
            timeout: SETTLE,
        },
        Step::SubmitText("/permissions bypass"),
        Step::WaitText {
            text: "Bypass ·",
            timeout: SETTLE,
        },
        Step::SubmitText("fixture gated reply"),
        Step::WaitText {
            text: "reply waiting for release",
            timeout: STREAM,
        },
        Step::Key(Key::Alt('m')),
        Step::Key(Key::Alt('m')),
        Step::WaitText {
            text: "the change applies after the current turn",
            timeout: SETTLE,
        },
        Step::AssertText("Bypass ·"),
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "Supervised ·",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "Type a message",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);

// Covers: an unported mutation gets a visible daemon-specific refusal and
// cannot write instructions or poison the next prompt. Checks the file after
// a later in-order model response, not a timing-based absence window.
// Owner: interactive TUI daemon command boundary
pub(super) const GATED_COMMAND: Scenario = Scenario::new(
    "daemon_gated_command",
    "Refuse unported commands without touching the local runtime or instructions",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::SubmitText("/remember this must not be written"),
        Step::WaitText {
            text: "/remember is not available with RHO_TUI_DAEMON yet",
            timeout: SETTLE,
        },
        Step::SubmitText("hello after refusal"),
        Step::WaitText {
            text: "fixture response: hello after refusal",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "Type a message",
            timeout: SETTLE,
        },
        Step::Custom(assert_instructions_unchanged),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(write_gate_config)
.with_env(ENV);

fn write_gate_config(home: &IsolatedHome) -> Result<()> {
    write_daemon_config(home)?;
    std::fs::write(home.workspace.join("AGENTS.md"), INSTRUCTIONS)?;
    Ok(())
}

fn assert_instructions_unchanged(harness: &mut PtyHarness) -> Result<()> {
    let workspace = harness.working_directory().context("missing workspace")?;
    ensure!(
        std::fs::read_to_string(workspace.join("AGENTS.md"))? == INSTRUCTIONS,
        "the refused /remember command changed project instructions"
    );
    Ok(())
}
