//! Plan approval hands the next turn to an execution policy lasting until exit.

use std::time::Duration;

use anyhow::{ensure, Context, Result};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    scenario::{Scenario, Step},
    PtyHarness,
};

fn setup(home: &IsolatedHome) -> Result<()> {
    std::fs::write(
        home.workspace.join(".plan-exit-config-before"),
        std::fs::read(&home.config_path)?,
    )?;
    Ok(())
}

fn assert_config_unchanged(harness: &mut PtyHarness) -> Result<()> {
    let workspace = harness
        .working_directory()
        .context("missing fixture workspace")?;
    let config = workspace
        .parent()
        .context("missing isolated home")?
        .join("home/.rho/config.toml");
    ensure!(
        std::fs::read(config)? == std::fs::read(workspace.join(".plan-exit-config-before"))?,
        "plan approval saved the until-exit permission mode to config"
    );
    Ok(())
}

fn assert_allow_edits(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let status = rows
        .iter()
        .rev()
        .find(|row| row.contains("gpt-5.5"))
        .context("missing status line")?;
    ensure!(
        status.contains("Allow edits"),
        "approval did not update status line: {status}"
    );
    assert_config_unchanged(harness)
}

fn assert_approval_feedback(harness: &mut PtyHarness) -> Result<()> {
    let receipt = harness
        .working_directory()
        .context("missing fixture workspace")?
        .join(".rho-fixture-plan-feedback");
    // This is the model-visible tool-result contract, not rendered UI copy.
    ensure!(
        std::fs::read_to_string(receipt)?.contains("include migration steps"),
        "approval feedback was absent from implementation-turn history"
    );
    Ok(())
}

fn assert_still_planning(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let status = rows
        .iter()
        .rev()
        .find(|row| row.contains("gpt-5.5"))
        .context("missing status line")?;
    ensure!(
        status.contains("Plan"),
        "keep planning escalated mode: {status}"
    );
    assert_no_implementation(harness)
}

fn assert_no_implementation(harness: &mut PtyHarness) -> Result<()> {
    let receipt = harness
        .working_directory()
        .context("missing fixture workspace")?
        .join(".rho-fixture-plan-implementation");
    ensure!(
        !receipt.exists(),
        "unapproved or failed plan queued implementation"
    );
    assert_config_unchanged(harness)
}

fn assert_plan_collapsed(harness: &mut PtyHarness) -> Result<()> {
    ensure!(
        !harness
            .screen()
            .contains_text("Verify the final safety step."),
        "long plan was not collapsed before expansion"
    );
    Ok(())
}

fn release_plan_child(harness: &mut PtyHarness) -> Result<()> {
    super::release_fixture(harness, ".rho-fixture-release-plan-child")
}

fn release_held_proposal(harness: &mut PtyHarness) -> Result<()> {
    super::release_fixture(harness, ".rho-fixture-release-plan-held")
}

// Covers: an Alt+M press queued during the proposal turn must not consume the
// approval; the approved mode applies and implementation still starts.
// Owner: turn-end permission ordering (PTY).
pub(super) const QUEUED_CYCLE_SCENARIO: Scenario = Scenario::new(
    "plan_exit_queued_cycle",
    "Apply an approved plan's mode over an Alt+M change queued during the proposal turn",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture plan exit held"),
        Step::WaitText {
            text: "Plan ready. How should Rho continue?",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Optional feedback for the plan",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        // The questionnaire owns keys until the tool completes; queue Alt+M
        // only once the proposal turn is waiting on the held reply.
        Step::WaitText {
            text: "✓ exit_plan_mode",
            timeout: STREAM,
        },
        Step::Key(Key::Alt('m')),
        Step::WaitText {
            text: "permission mode supervised queued for next turn",
            timeout: SETTLE,
        },
        Step::Custom(release_held_proposal),
        Step::WaitText {
            text: "fixture plan implementation reached",
            timeout: STREAM,
        },
        Step::Custom(assert_allow_edits),
        Step::ExitCommand,
        Step::Custom(assert_config_unchanged),
    ],
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);

// Covers: every turn driver must deliver the implementation handoff before
// evaluating the goal or returning to idle background delivery.
// Owner: interactive turn orchestration (PTY).
pub(super) const GOAL_SCENARIO: Scenario = Scenario::new(
    "plan_exit_goal",
    "Deliver plan approval from /goal before goal evaluation",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/goal fixture plan exit"),
        Step::WaitText {
            text: "Plan ready. How should Rho continue?",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Optional feedback for the plan",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "goal achieved",
            timeout: STREAM,
        },
        // The fixture evaluator returns Met only after the implementation
        // receipt is in model history; early evaluation deterministically blocks.
        Step::Custom(assert_allow_edits),
        Step::ExitCommand,
        Step::Custom(assert_config_unchanged),
    ],
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);

pub(super) const IDLE_COMPLETION_SCENARIO: Scenario = Scenario::new(
    "plan_exit_idle_completion",
    "Deliver plan approval from an idle delegated-completion turn",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture plan exit background"),
        Step::WaitText {
            text: "fixture plan background dispatched",
            timeout: STREAM,
        },
        Step::WaitQuiet {
            quiet_for: Duration::from_millis(250),
            timeout: SETTLE,
        },
        // Confirm the idle command path before releasing the child. During-
        // turn /permissions is unavailable, so an in-turn delivery cannot pass.
        Step::SubmitText("/permissions"),
        Step::WaitText {
            text: "permission mode: plan. usage:",
            timeout: STREAM,
        },
        Step::Custom(release_plan_child),
        Step::WaitText {
            text: "Plan ready. How should Rho continue?",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Optional feedback for the plan",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "fixture plan implementation reached",
            timeout: STREAM,
        },
        Step::Custom(assert_allow_edits),
        Step::ExitCommand,
        Step::Custom(assert_config_unchanged),
    ],
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);

// Covers: questionnaire key capture must not prevent reviewing a collapsed plan.
// Owner: interactive UX (PTY).
pub(super) const APPROVE_SCENARIO: Scenario = Scenario::new(
    "plan_exit_approve",
    "Expand a long plan while approving, then implement without saving config",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture plan exit long"),
        Step::WaitText {
            text: "Plan ready. How should Rho continue?",
            timeout: STREAM,
        },
        Step::Custom(assert_plan_collapsed),
        Step::Key(Key::Ctrl('o')),
        Step::WaitText {
            text: "Verify the final safety step.",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Optional feedback for the plan",
            timeout: STREAM,
        },
        Step::TypeText("include migration steps"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "fixture plan implementation reached",
            timeout: STREAM,
        },
        Step::Custom(assert_approval_feedback),
        Step::Custom(assert_allow_edits),
        Step::ExitCommand,
        Step::Custom(assert_config_unchanged),
    ],
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);

// Covers: reserved decision values typed as feedback must never grant permission.
// Owner: questionnaire authorization separation (PTY).
pub(super) const KEEP_SCENARIO: Scenario = Scenario::new(
    "plan_exit_keep_planning",
    "Decline a plan with reserved approval text as feedback without granting permission",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture plan exit"),
        Step::WaitText {
            text: "Plan ready. How should Rho continue?",
            timeout: STREAM,
        },
        Step::Key(Key::Down),
        Step::Key(Key::Down),
        Step::Key(Key::Down),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Optional feedback for the plan",
            timeout: STREAM,
        },
        Step::TypeText("bypass"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "fixture plan review complete",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "plan not approved; permission mode stays plan",
            timeout: STREAM,
        },
        // The boundary receipt guarantees the proposal turn is idle.
        Step::SubmitText("/permissions"),
        Step::WaitText {
            text: "permission mode: plan. usage:",
            timeout: STREAM,
        },
        Step::Custom(assert_still_planning),
        Step::ExitCommand,
        Step::Custom(assert_no_implementation),
    ],
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);

pub(super) const FAILED_SCENARIO: Scenario = Scenario::new(
    "plan_exit_failed_turn",
    "Discard approval when the proposal turn fails instead of handing off to implementation",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture plan exit fail"),
        Step::WaitText {
            text: "Plan ready. How should Rho continue?",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Optional feedback for the plan",
            timeout: STREAM,
        },
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "plan approval discarded; turn did not complete",
            timeout: STREAM,
        },
        Step::Custom(assert_still_planning),
        Step::ExitCommand,
        Step::Custom(assert_no_implementation),
    ],
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);
