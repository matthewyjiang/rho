//! Zen mode condenses tool cards into one summary row between output sections.

use std::{fs::OpenOptions, io::Write, time::Duration};

use anyhow::{ensure, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};

const SUMMARY_ROW: &str = "⋯ 1 tool call";

// Covers: a finished tool call in zen leaves a single summary row between the
// prompt and the follow-up answer instead of vanishing.
// Owner: interactive TUI transcript under zen display policy.
pub(super) const ZEN_TOOL_RUN_SUMMARY_SCENARIO: Scenario = Scenario::new(
    "zen_tool_run_summary",
    "Zen mode condenses a tool call into one summary row",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("run_tool_in_zen"),
        Step::SubmitText("fixture tool"),
        Step::WaitText {
            text: "tool lifecycle complete with one result",
            timeout: STREAM,
        },
        Step::WaitQuiet {
            quiet_for: Duration::from_millis(200),
            timeout: SETTLE,
        },
        Step::Custom(assert_summary_between_sections),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_zen);

fn setup_zen(home: &IsolatedHome) -> Result<()> {
    writeln!(
        OpenOptions::new().append(true).open(&home.config_path)?,
        "\n[display]\nzen_mode = true"
    )?;
    Ok(())
}

fn assert_summary_between_sections(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text(SUMMARY_ROW, SETTLE)?;
    let screen = harness.screen().contents();
    let prompt = screen.find("fixture tool");
    let summary = screen.find(SUMMARY_ROW);
    let answer = screen.find("tool lifecycle complete with one result");
    ensure!(
        matches!((prompt, summary, answer), (Some(prompt), Some(summary), Some(answer))
            if prompt < summary && summary < answer),
        "zen summary row must sit between the prompt and the answer: {screen}"
    );
    ensure!(
        screen.matches(SUMMARY_ROW).count() == 1,
        "zen must paint exactly one summary row: {screen}"
    );
    Ok(())
}
