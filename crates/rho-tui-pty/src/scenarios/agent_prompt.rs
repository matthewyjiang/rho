//! Covers: streaming agent cards lose the prompt beginning, cannot reveal the
//! hidden tail, or reset expansion on later deltas or launch.
//! Owner: interactive UX; existing agent scenarios wait only for completed launches.

use anyhow::{ensure, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

pub(super) const AGENT_PROMPT_SCENARIO: Scenario = Scenario::new(
    "agent_prompt_streaming",
    "Keep the multiline prompt prefix collapsed and preserve expansion through streaming and launch",
    // Narrow enough to wrap the first instruction; tall enough to keep the
    // expanded card, launch receipt, and running-child rail together in view.
    PtySize { rows: 40, cols: 64 },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(exercise_prompt),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup);

fn setup(home: &IsolatedHome) -> Result<()> {
    let mut config = std::fs::read_to_string(&home.config_path)?;
    // Three display rows expose the wrapped first instruction but must hide
    // the tail. This deliberately differs from the default cap.
    config.push_str("\n[display]\nmax_tool_output_lines = 3\noutput_streaming = \"live\"\n");
    std::fs::write(&home.config_path, config)?;
    Ok(())
}

fn assert_prefix(harness: &PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let first = rows.iter().position(|row| row.contains("prompt-prefix"));
    let wrapped = rows.iter().position(|row| row.contains("wrapped-prefix"));
    ensure!(
        first
            .zip(wrapped)
            .is_some_and(|(first, wrapped)| first < wrapped),
        "the beginning and wrapped continuation must remain on distinct rows:\n{}",
        harness.screen().contents()
    );
    Ok(())
}

fn assert_tail_hidden(harness: &PtyHarness, tail: &str) -> Result<()> {
    ensure!(
        !harness.screen().contains_text(tail),
        "collapsed prompt exposed {tail}:\n{}",
        harness.screen().contents()
    );
    assert_prefix(harness)
}

fn exercise_prompt(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("collapsed_streaming_prefix");
    harness.submit_text("fixture agent prompt")?;
    // Usage follows arguments on the same ordered event stream. The provider
    // stays parked here until release, so hidden-tail checks cannot race it.
    harness.wait_for_text("$1.000", STREAM)?;
    harness.wait_for_text("prompt-prefix", SETTLE)?;
    assert_tail_hidden(harness, "prompt-tail-one")?;

    harness.set_phase("expand_during_streaming");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text("prompt-tail-one", SETTLE)?;
    assert_prefix(harness)?;

    harness.set_phase("expanded_continuation");
    super::release_fixture(harness, ".rho-fixture-release-agent-prompt")?;
    harness.wait_for_text("$2.000", STREAM)?;
    harness.wait_for_text("prompt-tail-two", SETTLE)?;
    assert_prefix(harness)?;

    harness.set_phase("collapse_continued_streaming_prompt");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text_gone("prompt-tail-two", SETTLE)?;
    assert_tail_hidden(harness, "prompt-tail-one")?;
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text("prompt-tail-two", SETTLE)?;
    assert_prefix(harness)?;

    harness.set_phase("expanded_launch");
    super::release_fixture(harness, ".rho-fixture-release-agent-prompt")?;
    harness.wait_for_text("agent prompt launched", STREAM)?;
    harness.wait_for_text("prompt-tail-two", SETTLE)?;
    assert_prefix(harness)?;

    harness.set_phase("collapse_and_reexpand_launched_prompt");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text_gone("prompt-tail-two", SETTLE)?;
    assert_tail_hidden(harness, "prompt-tail-one")?;
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text("prompt-tail-two", SETTLE)?;
    assert_prefix(harness)?;
    Ok(())
}
