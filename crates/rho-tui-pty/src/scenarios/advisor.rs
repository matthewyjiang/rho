//! Advisor mode: the `/advisor` command, the config row, and a live review.
//!
//! The isolated home has no credentials, so every model picker would be empty.
//! `XAI_API_KEY` gives these scenarios a provider whose models come from the
//! static catalog, which is enough for the advisor model picker to offer rows.

use std::time::{Duration, Instant};

use anyhow::{ensure, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    scenario::{Scenario, Step},
    WaitTimeout,
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

/// Makes `xai` an available auth, so model pickers list its static catalog.
const XAI_KEY_ENV: &[(&str, &str)] = &[("XAI_API_KEY", "fixture-xai-key")];

const ADVISOR_MODEL: &str = "xai/grok-4.5";

/// Config a user can reach by hand: the mode is on with no advisor model, so
/// nothing reviews the session.
fn setup_advisor_without_model(home: &IsolatedHome) -> Result<()> {
    write_config(home, /*advisor_mode*/ true, /*with_model*/ false)
}

/// Config with advisor mode on and a model, so the tool is offered from startup.
fn setup_advisor_ready(home: &IsolatedHome) -> Result<()> {
    write_config(home, /*advisor_mode*/ true, /*with_model*/ true)
}

fn write_config(home: &IsolatedHome, advisor_mode: bool, with_model: bool) -> Result<()> {
    let mut config = format!(
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

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
        harness.pump(Duration::from_millis(25));
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

/// The advisor scenarios, as `ALL_SCENARIOS` lists them.
pub(super) const ADVISOR_MISSING_MODEL_SCENARIO: Scenario = Scenario {
    id: "advisor_missing_model",
    description: "Warn about advisor mode saved without a model",
    size: DEFAULT_SIZE,
    setup: Some(setup_advisor_without_model),
    env: XAI_KEY_ENV,
    args: &[],
    steps: ADVISOR_MISSING_MODEL_STEPS,
    smoke: false,
};

pub(super) const ADVISOR_REVIEW_SCENARIO: Scenario = Scenario {
    id: "advisor_review",
    description: "Consult the advisor during a turn, then survive an advisor failure",
    size: DEFAULT_SIZE,
    setup: Some(setup_advisor_ready),
    env: XAI_KEY_ENV,
    args: &[],
    steps: ADVISOR_REVIEW_STEPS,
    smoke: false,
};

// Covers: advisor mode saved on with no advisor model must read as unusable
// instead of pretending to work.
// Owner: interactive TUI
const ADVISOR_MISSING_MODEL_STEPS: &[Step] = &[
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
    Step::ExitCommand,
];

// Covers: the executor's advisor call must reach the advisor model and bring its
// guidance back as a tool result, and an advisor failure must stay a tool error
// rather than end the turn.
// Owner: interactive TUI
const ADVISOR_REVIEW_STEPS: &[Step] = &[
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
    // The advice can render before the turn finishes. Wait for its completion
    // receipt so the failure prompt starts a new turn instead of racing into
    // steering.
    Step::WaitText {
        text: "Worked for",
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
