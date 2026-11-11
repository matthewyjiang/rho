//! With `RHO_TUI_DAEMON=1`, the TUI's prompts run in the daemon session:
//! the reply and tool card stream into the transcript, Esc cancels a
//! running turn, and the composer is back once the session rests (#1667).

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

const ENV: &[(&str, &str)] = &[
    ("RHO_TUI_DAEMON", "1"),
    ("OPENAI_API_KEY", "sk-test-matrix"),
];
/// The idle composer's placeholder; a running turn shows `esc abort`.
const IDLE_COMPOSER: &str = "Type a message";

// Covers: a prompt sent from the daemon-hosted TUI shows its user row once
// (the daemon's echo), streams the daemon's tool card and reply, and leaves
// the composer idle once the session rests.
// Owner: interactive TUI as a daemon client
pub(super) const DAEMON_PROMPT_SCENARIO: Scenario = Scenario::new(
    "daemon_prompt",
    "Send a prompt to the daemon session and stream its tool card and reply",
    DEFAULT_SIZE,
    DAEMON_PROMPT_STEPS,
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);

// Covers: Esc during a daemon turn cancels it in the daemon, the transcript
// says the model was interrupted, and the next prompt runs normally.
// Owner: interactive TUI as a daemon client
pub(super) const DAEMON_CANCEL_SCENARIO: Scenario = Scenario::new(
    "daemon_cancel",
    "Cancel a running daemon turn with Esc, then prompt again",
    DEFAULT_SIZE,
    DAEMON_CANCEL_STEPS,
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);

// Covers: the statusline's cost follows a response while the daemon still
// streams it, and the finished turn's cost counts that response once.
// Owner: interactive TUI as a daemon client
pub(super) const DAEMON_STREAM_COST_SCENARIO: Scenario = Scenario::new(
    "daemon_stream_cost",
    "Show a daemon turn's cost while its response streams",
    DEFAULT_SIZE,
    &[
        Step::Phase("startup"),
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(stream_cost),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);

const DAEMON_PROMPT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("prompt"),
    Step::SubmitText("fixture tool"),
    Step::WaitText {
        text: "tool lifecycle complete with one result",
        timeout: STREAM,
    },
    Step::WaitText {
        text: ".rho-tui-fixture-output.txt",
        timeout: SETTLE,
    },
    Step::Phase("idle"),
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    // Nothing of the reply's stream outlasts the turn: a spinner left
    // running would keep the screen changing.
    Step::WaitQuiet {
        quiet_for: std::time::Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::Custom(assert_one_user_row),
    Step::ExitCommand,
];

const DAEMON_CANCEL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_delay"),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "esc abort",
        timeout: SETTLE,
    },
    Step::Phase("cancel"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    Step::Phase("resubmit"),
    Step::SubmitText("hello after cancel"),
    Step::WaitText {
        text: "fixture response: hello after cancel",
        timeout: STREAM,
    },
    Step::WaitText {
        text: IDLE_COMPOSER,
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

/// The prompt shows once: as the daemon's echo, with no local row too.
fn assert_one_user_row(harness: &mut PtyHarness) -> Result<()> {
    let count = harness.screen().contents().matches("fixture tool").count();
    anyhow::ensure!(
        count == 1,
        "expected the prompt once, found it {count} times"
    );
    Ok(())
}

/// The fixture reports a dollar of usage with each of its four chunks and
/// holds the response open between them.
fn stream_cost(harness: &mut PtyHarness) -> Result<()> {
    const RELEASE: &str = ".rho-fixture-release-streaming-controls";
    harness.set_phase("streaming");
    harness.submit_text("fixture streaming controls")?;
    for cost in ["$1.000", "$2.000", "$3.000", "$4.000"] {
        harness.wait_for_text(cost, STREAM)?;
        anyhow::ensure!(
            harness.screen().contains_text("esc abort"),
            "{cost} showed only after the response ended"
        );
        super::release_fixture(harness, RELEASE)?;
    }
    harness.set_phase("finished");
    harness.wait_for_text("Buffered completion delivered.", STREAM)?;
    harness.wait_for_text(IDLE_COMPOSER, SETTLE)?;
    anyhow::ensure!(
        harness.screen().contains_text("$4.000"),
        "the finished turn's cost is not the four dollars its response reported"
    );
    Ok(())
}
