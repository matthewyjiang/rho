//! Streaming preferences share one durable setting across the menu and shortcut.

use anyhow::{ensure, Context, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

// Covers: off leaks partial output, paragraph leaks its unfinished suffix, or
// switching modes finalizes a split Markdown fence and corrupts continued output.
// Also retains startup/menu/shortcut persistence coverage in the same turn.
// Owner: interactive TUI. Ordered usage receipts synchronize hidden-output checks.
pub(super) const STREAMING_CONTROLS_SCENARIO: Scenario = Scenario::new(
    "streaming_controls",
    "Hold output by mode and preserve split Markdown across persisted menu and shortcut changes",
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
    let mut config = std::fs::read_to_string(&home.config_path)?;
    config.push_str("\n[display]\noutput_streaming = \"off\"\n");
    std::fs::write(&home.config_path, config)?;
    // Keep assertions independent of IsolatedHome's directory layout.
    std::fs::write(
        home.workspace.join(".streaming-config-path"),
        home.config_path
            .to_str()
            .context("config path is not UTF-8")?,
    )?;
    Ok(())
}

fn assert_saved_mode(harness: &PtyHarness, mode: &str) -> Result<()> {
    let workspace = harness.working_directory().context("missing workspace")?;
    let path = std::fs::read_to_string(workspace.join(".streaming-config-path"))?;
    let config = std::fs::read_to_string(path)?;
    let expected = format!("output_streaming = \"{mode}\"");
    ensure!(
        config.lines().any(|line| line == expected),
        "saved config does not contain {expected}"
    );
    Ok(())
}

fn cycle_in_menu(harness: &mut PtyHarness, mode: &str) -> Result<()> {
    harness.submit_text("/config")?;
    harness.wait_for_text("Config · saves automatically", SETTLE)?;
    harness.inject_key(&Key::Down)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("Config / Appearance", SETTLE)?;
    // Theme, Zen mode, Cache miss notices, then Output streaming.
    harness.inject_key(&Key::Down)?;
    harness.inject_key(&Key::Down)?;
    harness.inject_key(&Key::Down)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text(&format!("output streaming: {mode}"), SETTLE)?;
    assert_saved_mode(harness, mode)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text("Agent behavior", SETTLE)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text_gone("Config · saves automatically", SETTLE)?;
    Ok(())
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
    assert_saved_mode(harness, "live")?;
    harness.wait_for_text("Hidden off prefix", SETTLE)?;

    harness.set_phase("paragraph_boundary");
    cycle_in_menu(harness, "paragraph")?;
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
    assert_saved_mode(harness, "off")?;
    harness.wait_for_text("Held paragraph suffix", SETTLE)?;

    harness.set_phase("switch_preserves_opening_fence");
    super::release_fixture(harness, ".rho-fixture-release-streaming-controls")?;
    harness.wait_for_text("$3.000", STREAM)?;
    harness.inject_key(&Key::Alt('s'))?;
    harness.wait_for_text("output streaming: live", SETTLE)?;
    assert_saved_mode(harness, "live")?;
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

    // The menu resumes from the shortcut's setting; completion flushes off mode.
    cycle_in_menu(harness, "paragraph")?;
    cycle_in_menu(harness, "off")?;
    harness.set_phase("finish_buffered_response");
    super::release_fixture(harness, ".rho-fixture-release-streaming-controls")?;
    harness.wait_for_text("Buffered completion delivered.", STREAM)?;
    harness.wait_for_text("Hidden off prefix continued.", SETTLE)?;
    assert_saved_mode(harness, "off")?;
    Ok(())
}
