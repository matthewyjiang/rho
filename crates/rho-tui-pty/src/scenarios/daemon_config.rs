//! `/config` and `/advisor` change settings through the daemon (#1720).

use std::time::Duration;

use anyhow::{ensure, Result};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

const ENV: &[(&str, &str)] = &[
    ("RHO_TUI_DAEMON", "1"),
    ("OPENAI_API_KEY", "sk-test-matrix"),
    ("XAI_API_KEY", "fixture-xai-key"),
];
const ROOT_TITLE: &str = "Config · saves automatically";
/// Zen's stand-in for the fixture tool's card.
const ZEN_SUMMARY: &str = "⋯ 1 tool call";

/// Opens `/config`, then the settings category whose row `filter` finds.
pub(super) fn open_category(harness: &mut PtyHarness, filter: &str, row: &str) -> Result<()> {
    harness.submit_text("/config")?;
    harness.wait_for_text(ROOT_TITLE, SETTLE)?;
    harness.type_into_field(filter)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text(row, SETTLE)
}

/// Back out of a category page and the root.
pub(super) fn close_config(harness: &mut PtyHarness) -> Result<()> {
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text(ROOT_TITLE, SETTLE)?;
    harness.inject_key(&Key::Esc)?;
    harness.wait_for_text_gone(ROOT_TITLE, SETTLE)
}

/// Flips the Display setting `filter` finds with Space; `status` is the
/// status the write reports.
pub(super) fn toggle_display_setting(
    harness: &mut PtyHarness,
    filter: &str,
    status: &str,
) -> Result<()> {
    open_category(harness, "display", "Zen mode")?;
    harness.type_into_field(filter)?;
    harness.inject_key(&Key::Char(' '))?;
    harness.wait_for_text(status, SETTLE)?;
    close_config(harness)
}

fn zen_condenses_the_tool_card(harness: &mut PtyHarness) -> Result<()> {
    ensure!(
        !harness.screen().contains_text(ZEN_SUMMARY),
        "zen is off, so the tool card shows in full:\n{}",
        harness.screen().contents()
    );
    toggle_display_setting(harness, "zen", "zen mode: on")?;
    // Only the daemon's change notification applies the setting, so this
    // fails unless the write round-trips and re-renders the transcript.
    harness.wait_for_text(ZEN_SUMMARY, SETTLE)
}

fn set_compact_threshold(harness: &mut PtyHarness) -> Result<()> {
    open_category(harness, "context", "Compact threshold")?;
    harness.type_into_field("threshold")?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("edit compact threshold (1–100 %)", SETTLE)?;
    harness.inject_key(&Key::End)?;
    for _ in 0..3 {
        harness.inject_key(&Key::Backspace)?;
    }
    harness.type_into_field("61")?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("compact threshold: 61%; applies next turn", SETTLE)?;
    // The row's badge changes once the daemon saved it and said so.
    harness.wait_for_text("61%", SETTLE)?;
    harness.wait_for_text("Applies from the next turn.", SETTLE)?;
    close_config(harness)
}

// Covers: a `tui` client setting changed in `/config` saves through the daemon
// and takes effect live (zen re-renders the transcript), and an agent setting
// shows its saved value with when it applies. Fails if `/config` edits only
// local state, or if a change never comes back from the daemon.
// Owner: interactive TUI settings screen as a daemon client
pub(super) const DAEMON_CONFIG: Scenario = Scenario::new(
    "daemon_config",
    "Toggle zen live and set the compact threshold through the daemon's settings",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture tool"),
        Step::WaitText {
            text: "tool lifecycle complete with one result",
            timeout: STREAM,
        },
        Step::WaitQuiet {
            quiet_for: Duration::from_millis(200),
            timeout: SETTLE,
        },
        Step::Phase("zen_through_the_daemon"),
        Step::Custom(zen_condenses_the_tool_card),
        Step::Phase("compact_threshold"),
        Step::Custom(set_compact_threshold),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);

// Covers: `/advisor on` with no advisor model asks for one, then turns the
// advisor on with it in one write, and the loaded session's next turn gets
// it. Fails if the change never reaches the session.
// Owner: interactive TUI `/advisor` as a daemon client
pub(super) const DAEMON_ADVISOR: Scenario = Scenario::new(
    "daemon_advisor",
    "Turn the advisor on with a model through the daemon and see the next turn take it",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::SubmitText("/advisor on"),
        Step::WaitText {
            text: "Advisor model · Type to search",
            timeout: SETTLE,
        },
        Step::TypeIntoField("grok-4.5"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "advisor: on; applies next turn",
            timeout: SETTLE,
        },
        Step::SubmitText("fixture stream"),
        Step::WaitText {
            text: "advisor mode on",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "assistant stream part one",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);
