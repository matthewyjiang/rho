//! `display.show_header_hints` hides the session header hint block at startup,
//! and the Appearance toggle brings it back live.

use std::{fs::OpenOptions, io::Write};

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP};

/// A ready-session hint line; present only while header hints are shown.
const HINT_TEXT: &str = "Show available commands";

// Covers: config-disabled header hints stay off at startup, and the /config
// Appearance toggle restores them without a restart.
// Owner: interactive TUI. The header cache must rebuild on the toggle.
pub(super) const HEADER_HINTS_SCENARIO: Scenario = Scenario::new(
    "header_hints_toggle",
    "Hide header hints from config at startup, then show them from Appearance",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("hidden_at_startup"),
        Step::Custom(assert_hints_hidden),
        Step::Phase("shown_after_toggle"),
        Step::Custom(toggle_hints_on),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_hidden_hints);

fn setup_hidden_hints(home: &IsolatedHome) -> Result<()> {
    writeln!(
        OpenOptions::new().append(true).open(&home.config_path)?,
        "\n[display]\nshow_header_hints = false"
    )?;
    Ok(())
}

fn assert_hints_hidden(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text_gone(HINT_TEXT, SETTLE)
}

fn toggle_hints_on(harness: &mut PtyHarness) -> Result<()> {
    harness.submit_text("/config")?;
    harness.wait_for_text("Config · saves automatically", SETTLE)?;
    harness.inject_key(&Key::Down)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("Config / Appearance", SETTLE)?;
    // Theme, Zen, Cache miss notices, Output streaming, Show reasoning output,
    // then Header hints.
    for _ in 0..5 {
        harness.inject_key(&Key::Down)?;
    }
    harness.inject_key(&Key::Char(' '))?;
    harness.wait_for_text("header hints: shown", SETTLE)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text("Config · saves automatically", SETTLE)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text_gone("Config · saves automatically", SETTLE)?;
    harness.wait_for_text(HINT_TEXT, SETTLE)
}
