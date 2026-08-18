//! Advisor mode: the `/advisor` command, the config row, and a live review.
//!
//! The isolated home has no credentials, so every model picker would be empty.
//! `XAI_API_KEY` gives these scenarios a provider whose models come from the
//! static catalog, which is enough for the advisor model picker to offer rows.

use std::time::{Duration, Instant};

use anyhow::{ensure, Result};

use crate::{env::IsolatedHome, harness::PtyHarness, keys::Key, scenario::Step, WaitTimeout};

use super::{SETTLE, STARTUP, STREAM};

/// Makes `xai` an available auth, so model pickers list its static catalog.
pub(super) const XAI_KEY_ENV: &[(&str, &str)] = &[("XAI_API_KEY", "fixture-xai-key")];

const ADVISOR_MODEL: &str = "xai/grok-4.5";

/// Config a user can reach by hand: the mode is on with no advisor model, so
/// nothing reviews the session.
pub(super) fn setup_advisor_without_model(home: &IsolatedHome) -> Result<()> {
    write_config(home, /*advisor_mode*/ true, /*with_model*/ false)
}

/// Config with advisor mode on and a model, so the tool is offered from startup.
pub(super) fn setup_advisor_ready(home: &IsolatedHome) -> Result<()> {
    write_config(home, /*advisor_mode*/ true, /*with_model*/ true)
}

fn write_config(home: &IsolatedHome, advisor_mode: bool, with_model: bool) -> Result<()> {
    let mut config = format!(
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"

[behavior]
credential_store = "file"
advisor_mode = {advisor_mode}
"#
    );
    if with_model {
        config.push_str(
            r#"
[internal_agents.advisor]
provider = "xai"
model = "grok-4.5"
auth = "xai-api-key"
"#,
        );
    }
    std::fs::write(&home.config_path, config)?;
    Ok(())
}

/// The composer top divider, the row of `─` immediately above the prompt.
fn top_composer_divider(harness: &PtyHarness) -> Result<String> {
    let rows = harness.screen().rows_text();
    let composer = rows.iter().position(|row| {
        let trimmed = row.trim_start();
        trimmed.starts_with("> ") || trimmed.contains("Type a message")
    });
    let Some(composer) = composer else {
        return Err(anyhow::anyhow!(
            "composer prompt not found:\n{}",
            harness.screen().debug_dump()
        ));
    };
    rows[..composer]
        .iter()
        .rev()
        .find(|row| is_rule_row(row))
        .map(|row| row.to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "composer top divider not found:\n{}",
                harness.screen().debug_dump()
            )
        })
}

fn is_rule_row(row: &str) -> bool {
    row.trim_start().starts_with('─')
}

/// The statusline row, found by the permission field that never drops.
fn status_row(harness: &PtyHarness) -> Result<String> {
    harness
        .screen()
        .rows_text()
        .iter()
        .rev()
        .find(|row| row.contains("Bypass"))
        .map(|row| row.trim().to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "statusline row not found:\n{}",
                harness.screen().debug_dump()
            )
        })
}

/// Polls the top composer divider until `check` passes or `timeout` elapses.
///
/// The toast can land a frame before the divider repaints after a mode change.
/// One-shot reads flake on slower PTY hosts; wait for the row.
fn wait_for_advisor_divider(
    harness: &mut PtyHarness,
    timeout: WaitTimeout,
    check: impl Fn(&str) -> bool,
    failure: impl FnOnce(&str) -> String,
) -> Result<()> {
    let deadline = Instant::now() + timeout.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        let last = top_composer_divider(harness).unwrap_or_default();
        if check(&last) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "{}\n{}",
                failure(&last),
                harness.screen().debug_dump()
            ));
        }
    }
}

fn assert_statusline_has_no_advisor(harness: &PtyHarness) -> Result<()> {
    let row = status_row(harness)?;
    ensure!(
        !row.contains("advisor"),
        "advisor must not remain on the statusline:\n{row}\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

fn assert_advisor_indicator_absent(harness: &mut PtyHarness) -> Result<()> {
    wait_for_advisor_divider(
        harness,
        SETTLE,
        |row| !row.is_empty() && !row.contains("advisor"),
        |row| {
            format!(
                "advisor mode is off, so the composer divider must not claim a reviewer:\n{row}"
            )
        },
    )?;
    assert_statusline_has_no_advisor(harness)
}

fn assert_advisor_indicator_names_the_model(harness: &mut PtyHarness) -> Result<()> {
    let expected = format!("advisor: {ADVISOR_MODEL}");
    wait_for_advisor_divider(
        harness,
        SETTLE,
        move |row| row.contains(&expected),
        |row| format!("composer divider must name the model reviewing the session:\n{row}"),
    )?;
    assert_statusline_has_no_advisor(harness)
}

fn assert_advisor_indicator_warns_about_the_missing_model(harness: &mut PtyHarness) -> Result<()> {
    wait_for_advisor_divider(
        harness,
        SETTLE,
        |row| row.contains("advisor: no model"),
        |row| format!("advisor mode with no model must read as unusable:\n{row}"),
    )?;
    assert_statusline_has_no_advisor(harness)
}

/// The advisor takes no arguments, so its card is the verb alone.
fn assert_advisor_card_has_no_empty_arguments(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    ensure!(
        !screen.contains("{}"),
        "the no-argument advisor tool must not render an empty argument object:\n{screen}"
    );
    Ok(())
}

/// The advisor has no conversation-model fallback, so offering that row would
/// promise a selection the mode cannot use.
fn assert_picker_omits_the_conversation_model(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    ensure!(
        !screen.contains("Use conversation model"),
        "the advisor model picker must not offer the conversation model:\n{screen}"
    );
    Ok(())
}

// Covers: /advisor on asks for a model, remembers it across off and on, and
// every surface that shows the mode follows the change.
// Owner: interactive TUI
pub(super) const ADVISOR_COMMAND_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Custom(assert_advisor_indicator_absent),
    Step::Phase("dismiss_model_prompt"),
    Step::SubmitText("/advisor on"),
    Step::WaitText {
        text: "select model for advisor",
        timeout: SETTLE,
    },
    Step::Custom(assert_picker_omits_the_conversation_model),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "advisor mode stays off",
        timeout: SETTLE,
    },
    Step::Custom(assert_advisor_indicator_absent),
    Step::Phase("choose_advisor_model"),
    Step::SubmitText("/advisor on"),
    Step::WaitText {
        text: "select model for advisor",
        timeout: SETTLE,
    },
    Step::TypeText("grok-4.5"),
    Step::WaitText {
        text: ADVISOR_MODEL,
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "advisor mode is on: xai/grok-4.5 reviews the session",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "advisor: xai/grok-4.5",
        timeout: SETTLE,
    },
    Step::Custom(assert_advisor_indicator_names_the_model),
    Step::Phase("config_row_turns_it_off"),
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Agent behavior",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Advisor mode"),
    Step::AssertText("on · xai/grok-4.5"),
    Step::TypeText("advisor_mode"),
    Step::WaitTextGone {
        text: "Permission mode",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "advisor mode is off",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Appearance",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Type a message",
        timeout: SETTLE,
    },
    Step::Custom(assert_advisor_indicator_absent),
    Step::Phase("model_survives_the_round_trip"),
    // The advisor model was chosen once, so turning the mode on again must not
    // ask for it a second time.
    Step::SubmitText("/advisor on"),
    Step::WaitText {
        text: "advisor mode is on: xai/grok-4.5 reviews the session",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "advisor: xai/grok-4.5",
        timeout: SETTLE,
    },
    Step::Custom(assert_advisor_indicator_names_the_model),
    Step::ExitCommand,
];

// Covers: advisor mode saved on with no advisor model must read as unusable and
// route the user to a model instead of pretending to work.
// Owner: interactive TUI
pub(super) const ADVISOR_MISSING_MODEL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "advisor: no model",
        timeout: STARTUP,
    },
    Step::Custom(assert_advisor_indicator_warns_about_the_missing_model),
    Step::Phase("command_asks_for_a_model"),
    Step::SubmitText("/advisor on"),
    Step::WaitText {
        text: "select model for advisor",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Type a message",
        timeout: SETTLE,
    },
    Step::Custom(assert_advisor_indicator_warns_about_the_missing_model),
    Step::Phase("config_row_asks_for_a_model"),
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Agent behavior",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("on · no model"),
    Step::TypeText("advisor_mode"),
    Step::WaitTextGone {
        text: "Permission mode",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "advisor mode is off",
        timeout: SETTLE,
    },
    // Turning it back on from the config row has no model to use, so the row
    // opens the picker rather than saving a mode that cannot run.
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "select model for advisor",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Advisor mode"),
    Step::Key(Key::Esc),
    Step::Key(Key::Esc),
    Step::ExitCommand,
];

// Covers: the executor's advisor call must reach the advisor model and bring its
// guidance back as a tool result, and an advisor failure must stay a tool error
// rather than end the turn.
// Owner: interactive TUI
pub(super) const ADVISOR_REVIEW_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "advisor: xai/grok-4.5",
        timeout: STARTUP,
    },
    Step::Custom(assert_advisor_indicator_names_the_model),
    Step::Phase("advice_reaches_the_executor"),
    Step::SubmitText("fixture advisor"),
    Step::WaitText {
        text: "advisor guidance: land the smallest change first",
        timeout: STREAM,
    },
    Step::Custom(assert_advisor_card_has_no_empty_arguments),
    Step::WaitText {
        text: "advisor consulted (advice)",
        timeout: STREAM,
    },
    Step::Phase("advisor_failure_stays_a_tool_error"),
    Step::SubmitText("fixture advisor failure"),
    Step::WaitText {
        text: "the advisor request failed",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "advisor consulted (error)",
        timeout: STREAM,
    },
    Step::ExitCommand,
];
