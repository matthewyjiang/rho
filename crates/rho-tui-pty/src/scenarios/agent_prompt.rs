//! Covers: streaming agent cards lose the prompt beginning or expansion, and
//! long prompts hide launch IDs or finished results in collapsed cards.
//! Owner: interactive UX; existing agent scenarios do not exercise long prompts.

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
    "Preserve streaming prompt expansion and show launch IDs and finished results before long prompts",
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
    Ok(())
}

fn launch_run_id(harness: &PtyHarness) -> Result<String> {
    let screen = harness.screen().contents();
    // Scope the receipt to the launch card, excluding the child rail and the
    // assistant response. Run IDs follow the real six-hex-digit contract.
    let card = screen
        .split_once("running in background")
        .and_then(|(_, body)| body.split_once("agent prompt launched"))
        .map(|(card, _)| card)
        .ok_or_else(|| anyhow::anyhow!("launch card missing:\n{screen}"))?;
    let id = card
        .split_whitespace()
        .find(|word| word.len() == 6 && word.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| anyhow::anyhow!("launch card hid the run ID:\n{screen}"))?;
    Ok(id.to_string())
}

fn exercise_prompt(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("collapsed_streaming_prefix");
    harness.submit_text("fixture agent prompt")?;
    // Usage follows arguments on the same ordered event stream. The provider
    // stays parked here until release, so hidden-tail checks cannot race it.
    harness.wait_for_text("$1.000", STREAM)?;
    harness.wait_for_text("prompt-prefix", SETTLE)?;
    assert_prefix(harness)?;
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
    assert_prefix(harness)?;
    assert_tail_hidden(harness, "prompt-tail-one")?;
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text("prompt-tail-two", SETTLE)?;
    assert_prefix(harness)?;

    harness.set_phase("expanded_launch");
    super::release_fixture(harness, ".rho-fixture-release-agent-prompt")?;
    harness.wait_for_text("agent prompt launched", STREAM)?;
    harness.wait_for_text("prompt-tail-two", SETTLE)?;
    assert_prefix(harness)?;

    let run_id = launch_run_id(harness)?;
    harness.set_phase("collapse_and_reexpand_launched_prompt");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text_gone("prompt-tail-two", SETTLE)?;
    assert_tail_hidden(harness, "prompt-tail-one")?;
    ensure!(
        launch_run_id(harness)? == run_id,
        "collapsed launch lost its run ID:\n{}",
        harness.screen().contents()
    );
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text("prompt-tail-two", SETTLE)?;
    assert_prefix(harness)?;

    harness.set_phase("collapsed_finished_result");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text_gone("prompt-tail-two", SETTLE)?;
    harness.submit_text("fixture agent prompt finished")?;
    // The provider's final response follows the real failed ToolCall snapshot,
    // so the completed result is durable before inspecting the collapsed card.
    harness.wait_for_text("agent prompt result received", STREAM)?;
    harness.wait_for_text("unknown agent 'absent'", SETTLE)?;
    assert_tail_hidden(harness, "finished-prompt-tail")?;

    harness.set_phase("expanded_finished_prompt");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text("finished-prompt-tail", SETTLE)?;
    harness.wait_for_text("finished-prompt-prefix", SETTLE)?;
    harness.wait_for_text("wrapped-finished-prefix", SETTLE)?;
    Ok(())
}
