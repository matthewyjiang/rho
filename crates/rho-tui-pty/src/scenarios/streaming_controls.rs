//! Streaming preferences share one durable setting across the menu and shortcut.

use anyhow::{ensure, Context, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

// Covers: startup honors the saved preference; menu and key changes save the
// same setting during a turn without steering, cancelling, or losing output.
// Owner: interactive TUI. The provider stays gated until the controls finish.
pub(super) const STREAMING_CONTROLS_SCENARIO: Scenario = Scenario::new(
    "streaming_controls",
    "Persist streaming preferences through the config menu and shortcut during a gated reply",
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
    // Off -> live proves startup loaded the non-default preference.
    harness.inject_key(&Key::Alt('s'))?;
    harness.wait_for_text("output streaming: live", SETTLE)?;
    assert_saved_mode(harness, "live")?;

    harness.submit_text("fixture gated reply")?;
    harness.wait_for_text("reply waiting for release", STREAM)?;
    harness.set_phase("menu_during_turn");
    cycle_in_menu(harness, "paragraph")?;
    harness.wait_for_text("reply waiting for release", SETTLE)?;

    harness.set_phase("key_during_turn");
    for mode in ["off", "live", "paragraph"] {
        harness.inject_key(&Key::Alt('s'))?;
        harness.wait_for_text(&format!("output streaming: {mode}"), SETTLE)?;
        assert_saved_mode(harness, mode)?;
        harness.wait_for_text("reply waiting for release", SETTLE)?;
    }
    // Reopening the menu must continue from the shortcut's saved preference.
    cycle_in_menu(harness, "off")?;
    harness.set_phase("finish_buffered_response");
    super::release_fixture(harness, ".rho-fixture-release-reply")?;
    harness.wait_for_text("reply completed after release", STREAM)?;
    harness.wait_for_text("reply waiting for release", SETTLE)?;
    assert_saved_mode(harness, "off")?;
    Ok(())
}
