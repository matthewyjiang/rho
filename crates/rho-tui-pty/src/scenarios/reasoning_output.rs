//! Reasoning visibility changes must apply to already completed turns.

use std::{fs::OpenOptions, io::Write, time::Duration};

use anyhow::{ensure, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

// Covers: hidden streamed reasoning is retained and can be repeatedly revealed;
// zen overrides visibility without discarding the completed reasoning. Interleaved
// reasoning preserves answer order and a mid-answer receipt when hidden.
// Owner: interactive TUI. The ordinary stream fixture has no interleaving.
pub(super) const REASONING_OUTPUT_RETROACTIVE_SCENARIO: Scenario = Scenario::new(
    "reasoning_output_retroactive",
    "Reveal and hide completed reasoning through Appearance, including zen mode",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
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
.with_setup(setup_hidden_reasoning);

fn setup_hidden_reasoning(home: &IsolatedHome) -> Result<()> {
    writeln!(
        OpenOptions::new().append(true).open(&home.config_path)?,
        "\n[display]\nshow_reasoning_output = false\nzen_mode = false"
    )?;
    Ok(())
}

fn toggle_completed_reasoning(harness: &mut PtyHarness) -> Result<()> {
    assert_interleaved_order(harness, /*receipt_visible*/ true)?;
    for (phase, setting, visible) in [
        ("reveal_past_reasoning", AppearanceSetting::Reasoning, true),
        ("hide_past_reasoning", AppearanceSetting::Reasoning, false),
        (
            "reveal_past_reasoning_again",
            AppearanceSetting::Reasoning,
            true,
        ),
        ("zen_hides_past_reasoning", AppearanceSetting::Zen, false),
        (
            "leaving_zen_restores_past_reasoning",
            AppearanceSetting::Zen,
            true,
        ),
    ] {
        harness.set_phase(phase);
        harness.submit_text("/config")?;
        harness.wait_for_text("Config · saves automatically", SETTLE)?;
        harness.inject_key(&Key::Down)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("Config / Appearance", SETTLE)?;
        // Appearance rows use direct shortcuts, not a text filter.
        harness.inject_key(&Key::Down)?;
        match setting {
            AppearanceSetting::Reasoning => {
                harness.inject_key(&Key::Down)?;
                harness.inject_key(&Key::Down)?;
            }
            AppearanceSetting::Zen => {}
        }
        harness.inject_key(&Key::Char(' '))?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("Config · saves automatically", SETTLE)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text_gone("Config · saves automatically", SETTLE)?;
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
        let receipt_visible = match setting {
            AppearanceSetting::Reasoning => true,
            AppearanceSetting::Zen => visible,
        };
        assert_interleaved_order(harness, receipt_visible)?;
    }
    Ok(())
}

enum AppearanceSetting {
    Reasoning,
    Zen,
}

fn assert_interleaved_order(harness: &mut PtyHarness, receipt_visible: bool) -> Result<()> {
    if receipt_visible {
        harness.wait_for_text("Thought for", SETTLE)?;
    } else {
        harness.wait_for_text_gone("Thought for", SETTLE)?;
    }
    let screen = harness.screen().contents();
    let before = screen.find("assistant before reasoning");
    let after = screen.find("assistant after reasoning");
    ensure!(
        matches!((before, after), (Some(before), Some(after)) if before < after),
        "interleaved answer segments must remain in order: {screen}"
    );
    if receipt_visible {
        let receipt = screen.find("Thought for");
        ensure!(
            matches!((before, receipt, after), (Some(before), Some(receipt), Some(after))
                if before < receipt && receipt < after),
            "reasoning receipt must stay between answer segments: {screen}"
        );
    }
    Ok(())
}
