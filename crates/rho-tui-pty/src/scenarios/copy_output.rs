use anyhow::Result;

use crate::{
    harness::WaitTimeout,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
    PtyHarness,
};

// Reuse the conversation-tree scenario's startup, stream, and UI wait budgets.
const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");
const CLIPBOARD: &[u8] = b"\x1b]52;c;";
const FIRST_OUTPUT: &[u8] = b"\x1b]52;c;Zml4dHVyZSByZXNwb25zZTogY29weSBmaXJzdApzZWNvbmQgbGluZQ==";
const SECOND_OUTPUT: &[u8] = b"\x1b]52;c;Zml4dHVyZSByZXNwb25zZTogY29weSBzZWNvbmQ=";

// Covers: choosing an earlier output must copy that exact text, not the latest
// response or its preview; opening/cancelling must not write the clipboard.
// Owner: interactive UX, including unsaved history and during-turn dispatch.
pub(super) const COPY_OUTPUT_SCENARIO: Scenario = Scenario::new(
    "copy_output",
    "Choose and copy an earlier output without changing the conversation",
    PtySize::new(32, 120),
    &[Step::Custom(copy_outputs), Step::ExitCommand],
    /* smoke */ false,
)
.with_env(&[("SSH_TTY", "rho-pty-clipboard")])
.with_args(&["--no-save"]);

fn copy_outputs(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("gpt-5.5", STARTUP)?;
    harness.submit_text("/copy")?;
    harness.wait_for_text("no assistant message to copy", SETTLE)?;
    if harness.raw_sequence_occurrences(CLIPBOARD) != 0 {
        anyhow::bail!("/copy with no outputs changed the clipboard");
    }
    harness.submit_text("copy first\nsecond line")?;
    harness.wait_for_text("fixture response: copy first", STREAM)?;
    harness.submit_text("copy second")?;
    harness.wait_for_text("fixture response: copy second", STREAM)?;

    harness.set_phase("cancel_without_copying");
    let before = harness.raw_sequence_occurrences(CLIPBOARD);
    harness.submit_text("/copy")?;
    harness.wait_for_text("Copy output", SETTLE)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text_gone("Copy output", SETTLE)?;
    if harness.raw_sequence_occurrences(CLIPBOARD) != before {
        anyhow::bail!("opening or cancelling /copy changed the clipboard");
    }

    harness.set_phase("choose_earlier_output");
    harness.submit_text("/copy")?;
    harness.wait_for_text("Copy output", SETTLE)?;
    harness.inject_key(&Key::Up)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_raw_sequence_occurrences(FIRST_OUTPUT, 1, SETTLE)?;
    harness.wait_for_text_gone("Copy output", SETTLE)?;
    harness.wait_for_text("fixture response: copy second", SETTLE)?;

    harness.set_phase("default_to_latest_output");
    harness.submit_text("/copy")?;
    harness.wait_for_text("Copy output", SETTLE)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_raw_sequence_occurrences(SECOND_OUTPUT, 1, SETTLE)?;
    harness.wait_for_text_gone("Copy output", SETTLE)?;

    harness.set_phase("copy_while_running");
    harness.submit_text("fixture delay")?;
    harness.wait_for_text("partial assistant before cancellation", STREAM)?;
    harness.submit_text("/copy")?;
    harness.wait_for_text("Copy output", SETTLE)?;
    harness.type_text("copy first")?;
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_raw_sequence_occurrences(FIRST_OUTPUT, 2, SETTLE)?;
    harness.wait_for_text_gone("Copy output", SETTLE)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text("model interrupted", STREAM)
}
