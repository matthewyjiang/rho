//! The streaming shortcut changes how held output is revealed mid-turn.

use anyhow::{ensure, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

// Covers: off leaks partial output, paragraph leaks its unfinished suffix, or
// switching modes finalizes a split Markdown fence and corrupts continued output.
// Also covers the startup client setting and shortcut changes in one turn.
// Owner: interactive TUI. Ordered usage receipts synchronize hidden-output checks.
pub(super) const STREAMING_CONTROLS_SCENARIO: Scenario = Scenario::new(
    "streaming_controls",
    "Hold output by mode and preserve split Markdown across shortcut changes",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(exercise_controls),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup);

fn setup(home: &IsolatedHome) -> Result<()> {
    let mut settings = std::fs::read_to_string(&home.client_settings_path)?;
    settings.push_str("output_streaming = \"off\"\n");
    std::fs::write(&home.client_settings_path, settings)?;
    Ok(())
}

/// Presses the streaming shortcut once; `mode` is the one it moves to.
fn cycle_shortcut(harness: &mut PtyHarness, mode: &str) -> Result<()> {
    harness.inject_key(&Key::Alt('s'))?;
    harness.wait_for_text(&format!("output streaming: {mode}"), SETTLE)
}

fn exercise_controls(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("load_saved_preference");
    harness.submit_text("fixture streaming controls")?;
    harness.wait_for_text("$1.000", STREAM)?;
    ensure!(
        !harness.screen().contains_text("Hidden off prefix"),
        "off leaked partial output after the provider receipt"
    );
    // Off -> live proves startup loaded the saved preference and reveals held
    // text without requiring another provider event to drive the preview.
    harness.inject_key(&Key::Alt('s'))?;
    harness.wait_for_text("output streaming: live", SETTLE)?;
    harness.wait_for_text("Hidden off prefix", SETTLE)?;

    harness.set_phase("paragraph_boundary");
    cycle_shortcut(harness, "paragraph")?;
    super::release_fixture(harness, ".rho-fixture-release-streaming-controls")?;
    harness.wait_for_text("$2.000", STREAM)?;
    harness.wait_for_text("Paragraph checkpoint visible.", SETTLE)?;
    ensure!(
        !harness.screen().contains_text("Held paragraph suffix"),
        "paragraph mode leaked its incomplete suffix after the provider receipt"
    );

    harness.set_phase("switch_reveals_held_suffix");
    harness.inject_key(&Key::Alt('s'))?;
    harness.wait_for_text("output streaming: off", SETTLE)?;
    harness.wait_for_text("Held paragraph suffix", SETTLE)?;

    harness.set_phase("switch_preserves_opening_fence");
    super::release_fixture(harness, ".rho-fixture-release-streaming-controls")?;
    harness.wait_for_text("$3.000", STREAM)?;
    harness.inject_key(&Key::Alt('s'))?;
    harness.wait_for_text("output streaming: live", SETTLE)?;
    super::release_fixture(harness, ".rho-fixture-release-streaming-controls")?;
    harness.wait_for_text("$4.000", STREAM)?;
    harness.wait_for_text("Markdown continuation complete.", SETTLE)?;
    let rows = harness.screen().rows_text();
    let code_rows = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains("let streamed_value = 7;"))
        .collect::<Vec<_>>();
    ensure!(
        code_rows.len() == 1
            && code_rows[0].1.trim() == "let streamed_value = 7;"
            && code_rows[0]
                .0
                .checked_sub(1)
                .is_some_and(|header| { rows[header].split_whitespace().next() == Some("RUST") }),
        "split opening fence must render one Rust code block, not plain prose:\n{}",
        harness.screen().contents()
    );
    ensure!(
        !harness.screen().contains_text("```")
            && !harness
                .screen()
                .contains_text("**Markdown continuation complete."),
        "switching modes corrupted the fence state or rendered the prose preview as code"
    );

    // Completion flushes off mode.
    cycle_shortcut(harness, "paragraph")?;
    cycle_shortcut(harness, "off")?;
    harness.set_phase("finish_buffered_response");
    super::release_fixture(harness, ".rho-fixture-release-streaming-controls")?;
    harness.wait_for_text("Buffered completion delivered.", STREAM)?;
    harness.wait_for_text("Hidden off prefix continued.", SETTLE)?;
    Ok(())
}
