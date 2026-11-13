//! Reasoning visibility changes must apply to already completed turns.

use std::{fs::OpenOptions, io::Write, time::Duration};

use anyhow::{ensure, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    scenario::{Scenario, Step},
};

use super::{
    daemon_config::toggle_display_setting, daemon_resume::write_daemon_config, DEFAULT_SIZE,
    SETTLE, STARTUP, STREAM,
};

// Covers: hidden streamed reasoning is retained and can be repeatedly revealed
// from `/config`; zen overrides visibility without discarding the completed
// reasoning. Interleaved reasoning preserves answer order. The daemon client
// draws no reasoning receipt yet, so this asserts none.
// Owner: interactive TUI. The ordinary stream fixture has no interleaving.
pub(super) const REASONING_OUTPUT_RETROACTIVE_SCENARIO: Scenario = Scenario::new(
    "reasoning_output_retroactive",
    "Reveal and hide completed reasoning through /config, including zen mode",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::Phase("complete_turn_with_reasoning_hidden"),
        Step::SubmitText("fixture interleaved reasoning"),
        Step::WaitText {
            text: "assistant after reasoning",
            timeout: STREAM,
        },
        Step::WaitQuiet {
            quiet_for: Duration::from_millis(200),
            timeout: SETTLE,
        },
        Step::WaitTextGone {
            text: "deterministic reasoning phase",
            timeout: SETTLE,
        },
        Step::Custom(toggle_completed_reasoning),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup_hidden_reasoning)
.with_env(&[("OPENAI_API_KEY", "sk-test-matrix")]);

fn setup_hidden_reasoning(home: &IsolatedHome) -> Result<()> {
    write_daemon_config(home)?;
    writeln!(
        OpenOptions::new()
            .append(true)
            .open(&home.client_settings_path)?,
        "show_reasoning_output = false\nzen_mode = false"
    )?;
    Ok(())
}

fn toggle_completed_reasoning(harness: &mut PtyHarness) -> Result<()> {
    assert_interleaved_order(harness)?;
    // (phase, Display row filter, status the write reports, reasoning visible
    // after). `output` finds Show reasoning output first; Zen's help mentions
    // reasoning, so `reasoning` would find Zen.
    for (phase, filter, status, visible) in [
        (
            "reveal_past_reasoning",
            "output",
            "show reasoning output: on",
            true,
        ),
        (
            "hide_past_reasoning",
            "output",
            "show reasoning output: off",
            false,
        ),
        (
            "reveal_past_reasoning_again",
            "output",
            "show reasoning output: on",
            true,
        ),
        ("zen_hides_past_reasoning", "zen", "zen mode: on", false),
        (
            "leaving_zen_restores_past_reasoning",
            "zen",
            "zen mode: off",
            true,
        ),
    ] {
        harness.set_phase(phase);
        toggle_display_setting(harness, filter, status)?;
        harness.wait_for_text("assistant after reasoning", SETTLE)?;
        for reasoning in [
            "deterministic reasoning phase one",
            "deterministic reasoning phase two",
        ] {
            if visible {
                harness.wait_for_text(reasoning, SETTLE)?;
            } else {
                harness.wait_for_text_gone(reasoning, SETTLE)?;
            }
        }
        assert_interleaved_order(harness)?;
    }
    Ok(())
}

fn assert_interleaved_order(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let before = screen.find("assistant before reasoning");
    let after = screen.find("assistant after reasoning");
    ensure!(
        matches!((before, after), (Some(before), Some(after)) if before < after),
        "interleaved answer segments must remain in order: {screen}"
    );
    Ok(())
}
