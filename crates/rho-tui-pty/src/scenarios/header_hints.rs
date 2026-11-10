//! `show_header_hints` in `clients/tui.toml` hides the session header hint block at startup,
//! and the daemon `/config` toggle brings it back live.

use std::{fs::OpenOptions, io::Write};

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    scenario::{Scenario, Step},
};

use super::{
    daemon_config::toggle_display_setting, daemon_resume::write_daemon_config, DEFAULT_SIZE,
    SETTLE, STARTUP,
};

/// A ready-session hint line; present only while header hints are shown.
const HINT_TEXT: &str = "Show available commands";
/// The header's brand line, rendered regardless of hint visibility.
const BRAND_TEXT: &str = "rho  v";

// Covers: config-disabled header hints stay off at startup, and the /config
// Display toggle restores them without a restart.
// Owner: interactive TUI. The header cache must rebuild on the toggle.
pub(super) const HEADER_HINTS_SCENARIO: Scenario = Scenario::new(
    "header_hints_toggle",
    "Hide header hints from config at startup, then show them from /config",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
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
.with_setup(setup_hidden_hints)
.with_env(&[
    ("RHO_TUI_DAEMON", "1"),
    ("OPENAI_API_KEY", "sk-test-matrix"),
]);

fn setup_hidden_hints(home: &IsolatedHome) -> Result<()> {
    write_daemon_config(home)?;
    writeln!(
        OpenOptions::new()
            .append(true)
            .open(&home.client_settings_path)?,
        "show_header_hints = false"
    )?;
    Ok(())
}

fn assert_hints_hidden(harness: &mut PtyHarness) -> Result<()> {
    // The brand line proves the header rendered, so absence means hidden.
    harness.wait_for_text(BRAND_TEXT, SETTLE)?;
    harness.wait_for_text_gone(HINT_TEXT, SETTLE)
}

fn toggle_hints_on(harness: &mut PtyHarness) -> Result<()> {
    toggle_display_setting(harness, "header", "header hints: on")?;
    harness.wait_for_text(HINT_TEXT, SETTLE)
}
