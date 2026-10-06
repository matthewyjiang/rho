//! Standing instructions are appended locally, without starting an assistant turn.

use anyhow::{ensure, Context, Result};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    scenario::{Scenario, Step},
    PtyHarness,
};

pub(super) const REMEMBER_SCENARIO: Scenario = Scenario::new(
    "remember_instruction",
    "Append project and global instructions without a model turn",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("project_instruction"),
        Step::SubmitText("/remember use max jobs 8"),
        Step::WaitText {
            text: "remembered in",
            timeout: SETTLE,
        },
        Step::Custom(assert_first_instruction),
        Step::Phase("append_instruction"),
        Step::SubmitText("/remember   keep changes focused  "),
        // Both writes have the same notice. A subsequent usage response is a
        // durable barrier proving the second command finished, not the first.
        Step::SubmitText("/remember"),
        Step::WaitText {
            text: "usage: /remember [global] <text>",
            timeout: SETTLE,
        },
        Step::Custom(assert_appended_instructions),
        Step::Phase("global_instruction"),
        Step::SubmitText("/remember global be concise"),
        Step::WaitText {
            text: "remembered in ~/.rho/AGENTS.md",
            timeout: SETTLE,
        },
        Step::Custom(assert_global_instruction),
        Step::Phase("normal_prompt"),
        Step::SubmitText("remember follow-up"),
        Step::WaitText {
            text: "fixture response: remember follow-up",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
);

fn assert_first_instruction(harness: &mut PtyHarness) -> Result<()> {
    let workspace = harness.working_directory().context("missing workspace")?;
    let contents = std::fs::read_to_string(workspace.join("AGENTS.md"))?;
    ensure!(
        contents == "- use max jobs 8\n",
        "unexpected new AGENTS.md: {contents:?}"
    );
    ensure!(
        !harness.screen().contains_text("fixture response:"),
        "/remember started a model turn"
    );
    Ok(())
}

fn assert_appended_instructions(harness: &mut PtyHarness) -> Result<()> {
    let workspace = harness.working_directory().context("missing workspace")?;
    let contents = std::fs::read_to_string(workspace.join("AGENTS.md"))?;
    ensure!(
        contents == "- use max jobs 8\n- keep changes focused\n",
        "unexpected appended AGENTS.md: {contents:?}"
    );
    Ok(())
}

fn assert_global_instruction(harness: &mut PtyHarness) -> Result<()> {
    let root = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?;
    let contents = std::fs::read_to_string(root.join("home/.rho/AGENTS.md"))?;
    ensure!(
        contents == "- be concise\n",
        "unexpected global AGENTS.md: {contents:?}"
    );
    ensure!(
        !harness.screen().contains_text("fixture response:"),
        "/remember started a model turn"
    );
    Ok(())
}
