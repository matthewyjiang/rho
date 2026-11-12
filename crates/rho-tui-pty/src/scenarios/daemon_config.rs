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

/// Flips the Display setting `filter` finds with Enter (Space would type
/// into the filter); `status` is the status the write reports.
pub(super) fn toggle_display_setting(
    harness: &mut PtyHarness,
    filter: &str,
    status: &str,
) -> Result<()> {
    open_category(harness, "display", "Zen mode")?;
    harness.type_into_field(filter)?;
    harness.inject_key(&Key::Enter)?;
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
    // The status above is set as the write is sent; the row's own badge
    // changes only once the daemon saved it and said so.
    harness.wait_for_screen("the threshold row shows 61%", SETTLE, |screen| {
        screen
            .rows_text()
            .iter()
            .any(|row| row.contains("Compact threshold") && row.contains("61%"))
    })?;
    // A value past the range reaches the daemon, which refuses it.
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("edit compact threshold (1–100 %)", SETTLE)?;
    harness.inject_key(&Key::End)?;
    for _ in 0..2 {
        harness.inject_key(&Key::Backspace)?;
    }
    harness.type_into_field("101")?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("must be at most 100; got 101", SETTLE)?;
    close_config(harness)
}

/// The composer rule's vim mode marker.
const VIM_INSERT: &str = "INSERT";

fn editing_mode_and_its_default(harness: &mut PtyHarness) -> Result<()> {
    open_category(harness, "display", "Zen mode")?;
    harness.type_into_field("editing")?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("Editing mode · Type to search", SETTLE)?;
    harness.type_into_field("vim")?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("editing mode: Vim", SETTLE)?;
    close_config(harness)?;
    harness.wait_for_text(VIM_INSERT, SETTLE)?;

    // The page opens on the value in force, so Enter keeps vim.
    open_category(harness, "display", "Zen mode")?;
    harness.type_into_field("editing")?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("Default (Default)", SETTLE)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("editing mode: Vim", SETTLE)?;
    close_config(harness)?;
    harness.wait_for_text(VIM_INSERT, SETTLE)?;

    // Once the file sets it, the page offers the default back.
    open_category(harness, "display", "Zen mode")?;
    harness.type_into_field("editing")?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("Default (Default)", SETTLE)?;
    // `default` leaves the plain choice, then the reset row below it.
    harness.type_into_field("default")?;
    harness.inject_key(&Key::Down)?;
    harness.inject_key(&Key::Enter)?;
    harness.wait_for_text("editing mode: default", SETTLE)?;
    close_config(harness)?;
    harness.wait_for_text_gone(VIM_INSERT, SETTLE)
}

fn lowering_prompt_history_asks_first(harness: &mut PtyHarness) -> Result<()> {
    // Opens the limit's editor on the selected row and enters 10.
    let lower = |harness: &mut PtyHarness| -> Result<()> {
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("edit prompt history limit", SETTLE)?;
        harness.inject_key(&Key::End)?;
        for _ in 0..4 {
            harness.inject_key(&Key::Backspace)?;
        }
        harness.type_into_field("10")?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("Keep only the newest 10 prompts?", SETTLE)
    };
    open_category(harness, "display", "Zen mode")?;
    // A space in the filter types, rather than toggling a matching row.
    harness.type_into_field("prompt history")?;
    lower(harness)?;
    harness.inject_key(&Key::Char('n'))?;
    harness.wait_for_text_gone("Keep only the newest 10 prompts?", SETTLE)?;
    ensure!(
        !harness.screen().contains_text("prompt history limit: 10"),
        "declining sends nothing:\n{}",
        harness.screen().contents()
    );
    // Back on the page, filter and selection kept.
    lower(harness)?;
    harness.inject_key(&Key::Char('y'))?;
    harness.wait_for_text("prompt history limit: 10", SETTLE)?;

    // Raised past the default, an empty entry resets it to the default,
    // which lowers it again and so asks too.
    let enter_limit = |harness: &mut PtyHarness, text: &str| -> Result<()> {
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("edit prompt history limit", SETTLE)?;
        harness.inject_key(&Key::End)?;
        for _ in 0..8 {
            harness.inject_key(&Key::Backspace)?;
        }
        if !text.is_empty() {
            harness.type_into_field(text)?;
        }
        harness.inject_key(&Key::Enter)
    };
    enter_limit(harness, "2000")?;
    // Only the daemon's change notification moves the limit in force.
    harness.wait_for_screen("the limit row shows 2000", SETTLE, |screen| {
        screen
            .rows_text()
            .iter()
            .any(|row| row.contains("Prompt history limit") && row.contains("2000"))
    })?;
    enter_limit(harness, "")?;
    harness.wait_for_text("Keep only the newest 1000 prompts?", SETTLE)?;
    harness.inject_key(&Key::Char('y'))?;
    harness.wait_for_text("prompt history limit: default", SETTLE)?;
    close_config(harness)
}

// Covers: a `tui` client setting changed in `/config` saves through the daemon
// and takes effect live (zen re-renders the transcript; vim mode turns on, and
// off again from the page's default row); an agent setting shows its saved
// value once the daemon confirms it, and one the daemon refuses says why;
// lowering the prompt history limit asks before deleting prompts, a reset
// to a lower default included. Fails if
// `/config` edits only local state, or if a change never comes back from the
// daemon.
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
        Step::Phase("editing_mode"),
        Step::Custom(editing_mode_and_its_default),
        Step::Phase("prompt_history_limit"),
        Step::Custom(lowering_prompt_history_asks_first),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);

// Covers: `/advisor on` with no advisor model asks for one, Space typing into
// that page's filter rather than choosing a model, then turns the advisor on
// with it in one write, and the loaded session's next turn gets it;
// resetting its model turns it off in the same write, never leaving it on
// with no model; `/advisor off` turns it off. Fails if the change never reaches the session.
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
        Step::Phase("space_filters_a_model_page"),
        Step::Custom(space_types_into_the_model_filter),
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
        Step::Phase("advisor_model_reset"),
        Step::SubmitText("/advisor model"),
        Step::WaitText {
            text: "Advisor model · Type to search",
            timeout: SETTLE,
        },
        // Only the reset row reads `not set`; `default` also finds models.
        Step::TypeIntoField("not set"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "advisor: off, model reset; applies next turn",
            timeout: SETTLE,
        },
        // An open page re-renders once the daemon reports the write, and
        // the model and the mode change in that one report.
        Step::SubmitText("/advisor model"),
        Step::WaitText {
            text: "Advisor model · Type to search",
            timeout: SETTLE,
        },
        Step::WaitTextGone {
            text: "Default (not set)",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        // Off with no model, so turning it on asks for one; left on, the
        // toggle would turn it off instead.
        Step::SubmitText("/advisor"),
        Step::WaitText {
            text: "Advisor model · Type to search",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::Phase("advisor_off"),
        Step::SubmitText("/advisor off"),
        Step::WaitText {
            text: "advisor mode: off; applies next turn",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(write_daemon_config)
.with_env(ENV);

/// Space on a model page types into its filter rather than choosing the
/// highlighted model, then the filter clears again.
fn space_types_into_the_model_filter(harness: &mut PtyHarness) -> Result<()> {
    harness.type_into_field("grok ")?;
    harness.wait_for_quiet(Duration::from_millis(200), SETTLE)?;
    ensure!(
        harness.screen().contains_text("Advisor model")
            && !harness.screen().contains_text("advisor: on"),
        "Space chose a model:\n{}",
        harness.screen().contents()
    );
    for _ in 0.."grok ".len() {
        harness.inject_key(&Key::Backspace)?;
    }
    Ok(())
}
