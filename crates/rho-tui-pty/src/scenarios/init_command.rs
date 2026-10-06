//! Project onboarding writes instructions, then /new reloads the cached prompt.

use anyhow::{ensure, Context, Result};

use super::{
    assert_helpers::wait_for_turn_completion_after, DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    scenario::{Scenario, Step},
    PtyHarness,
};

const INSTRUCTIONS: &str =
    "# Project instructions\n\nUse fixture-init-project-rule when working in this repository.\n";

// Covers: /init must execute its initial skill and persist instructions; the
// existing session stays cached, while /new must load the newly written file.
// Owner: interactive TUI onboarding and session lifecycle.
pub(super) const INIT_COMMAND_SCENARIO: Scenario = Scenario::new(
    "init_command",
    "Create project instructions with /init and reload them on /new",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/init"),
        Step::WaitText {
            text: "fixture init wrote AGENTS.md",
            timeout: STREAM,
        },
        Step::Custom(assert_written_and_cached),
        Step::SubmitText("/new"),
        Step::WaitText {
            text: "new session",
            timeout: SETTLE,
        },
        Step::SubmitText("fixture init instructions"),
        Step::WaitText {
            text: "fixture init instructions loaded",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
);

// Covers: Plan must refuse onboarding before a model turn or any file write.
// Owner: interactive TUI permission gating.
pub(super) const INIT_PLAN_SCENARIO: Scenario = Scenario::new(
    "init_plan",
    "Refuse /init in Plan permission mode without creating instructions",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/init"),
        Step::WaitText {
            text: "file writes are denied in plan permission mode",
            timeout: SETTLE,
        },
        Step::Custom(assert_no_instructions),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_args(&["--permission-mode", "plan"]);

fn assert_written_and_cached(harness: &mut PtyHarness) -> Result<()> {
    let path = harness
        .working_directory()
        .context("missing workspace")?
        .join("AGENTS.md");
    ensure!(
        std::fs::read_to_string(path)? == INSTRUCTIONS,
        "/init did not persist the complete fixture instructions"
    );
    wait_for_turn_completion_after(harness, "fixture init wrote AGENTS.md")?;
    harness.submit_text("fixture init instructions")?;
    harness.wait_for_text("fixture init instructions missing", STREAM)?;
    wait_for_turn_completion_after(harness, "fixture init instructions missing")
}

fn assert_no_instructions(harness: &mut PtyHarness) -> Result<()> {
    let path = harness
        .working_directory()
        .context("missing workspace")?
        .join("AGENTS.md");
    ensure!(!path.try_exists()?, "Plan onboarding wrote instructions");
    Ok(())
}
