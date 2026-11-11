//! Plan approval hands the next turn to an execution policy lasting until exit.

use anyhow::{ensure, Context, Result};

use super::{
    assert_helpers::wait_for_turn_completion_after, DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
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

/// Typed feedback fills the questionnaire's free-text field, which runs
/// through the paste-burst detector. Its Enter suppression window starts when
/// Rho *reads* the last key, not when the harness wrote it: a stalled runner
/// can read all six keys late, so a settle timed from the harness side still
/// lands Enter inside the window and it becomes a newline. Wait for Rho to
/// render the text first, which proves it read every key, then settle.
fn submit_typed_feedback(harness: &mut PtyHarness, feedback: &str) -> Result<()> {
    harness.wait_for_text(&format!("other: {feedback}"), SETTLE)?;
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Enter)
}

fn submit_migration_feedback(harness: &mut PtyHarness) -> Result<()> {
    submit_typed_feedback(harness, "include migration steps")
}

fn submit_bypass_feedback(harness: &mut PtyHarness) -> Result<()> {
    submit_typed_feedback(harness, "bypass")
}

fn wait_for_dispatch_turn(harness: &mut PtyHarness) -> Result<()> {
    wait_for_turn_completion_after(harness, "fixture plan background dispatched")
}

fn wait_for_implementation_turn(harness: &mut PtyHarness) -> Result<()> {
    wait_for_turn_completion_after(harness, "fixture plan implementation reached")
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
    QUEUED_CYCLE_STEPS,
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);

/// Approves a held plan, queues Alt+M once the proposal turn runs again,
/// then checks the approved mode outlasts the queued one.
const QUEUED_CYCLE_STEPS: &[Step] = &[
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
    Step::Custom(wait_for_resumed_proposal_turn),
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
    // A queued change applies once the running work rests, which may
    // be after the implementation turn. The next turn runs after it, so
    // the mode this turn shows is final.
    Step::Custom(wait_for_implementation_turn),
    Step::SubmitText("fixture plan mode check"),
    Step::WaitText {
        text: "fixture plan received unrelated continuation",
        timeout: STREAM,
    },
    Step::Custom(assert_allow_edits),
    Step::ExitCommand,
    Step::Custom(assert_config_unchanged),
];

/// Waits for the resumed proposal turn's durable activity label, which the
/// held reply keeps up. In process, the next step's start writes a routine
/// status that clears any toast, so the wait outlasts the brief `starting`
/// label: a late step start then cannot wipe the queued-mode toast before it
/// paints. A daemon session's wire carries no step starts, so its turn stays
/// `starting`.
fn wait_for_resumed_proposal_turn(harness: &mut PtyHarness) -> Result<()> {
    let label = if harness.follows_daemon() {
        "starting ·"
    } else {
        "waiting for provider"
    };
    harness.wait_for_text(label, STREAM)
}

// Covers: an idle delegated-completion turn must deliver the implementation
// handoff before returning to idle background delivery.
// Owner: interactive turn orchestration (PTY).
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
        // /permissions is idle-only. Screen silence is not idleness here: the
        // background rail keeps redrawing, at 100ms or 500ms depending on
        // other pending UI work, so wait for this turn's own receipt instead.
        Step::Custom(wait_for_dispatch_turn),
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
    APPROVE_STEPS,
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"]);

// Covers: the same approval when the session runs in the daemon: the
// feedback typed with it reaches the implementation turn's history.
// Owner: interactive TUI as a daemon client
#[cfg(unix)]
pub(super) const DAEMON_APPROVE_SCENARIO: Scenario = Scenario::new(
    "daemon_plan_exit_approve",
    "Approve a daemon session's plan with feedback, then implement",
    DEFAULT_SIZE,
    APPROVE_STEPS,
    /*smoke*/ false,
)
.with_setup(setup)
.with_args(&["--permission-mode", "plan"])
.with_env(super::daemon_resume::DAEMON_ENV);

const APPROVE_STEPS: &[Step] = &[
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
    Step::Custom(submit_migration_feedback),
    Step::WaitText {
        text: "fixture plan implementation reached",
        timeout: STREAM,
    },
    Step::Custom(assert_approval_feedback),
    Step::Custom(assert_allow_edits),
    Step::ExitCommand,
    Step::Custom(assert_config_unchanged),
];

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
        Step::Custom(submit_bypass_feedback),
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

/// `base` with its TUI as a daemon client.
#[cfg(unix)]
const fn in_daemon(id: &'static str, description: &'static str, base: Scenario) -> Scenario {
    Scenario {
        id,
        description,
        env: super::daemon_resume::DAEMON_ENV,
        ..base
    }
}

// Covers: a plan proposed from a delegated child's completion hands off to
// implementation. The daemon holds the prompt open while a subagent runs (its
// idle wake covers only workflows and processes), so the completion arrives
// in that prompt rather than in an idle turn, and the idle-only
// `/permissions` probe does not apply.
// Owner: interactive TUI as a daemon client (#1667)
#[cfg(unix)]
pub(super) const DAEMON_DELEGATED_COMPLETION_SCENARIO: Scenario = Scenario::new(
    "daemon_plan_exit_delegated_completion",
    "Deliver a daemon session's plan approval from a delegated child's completion",
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
.with_args(&["--permission-mode", "plan"])
.with_env(super::daemon_resume::DAEMON_ENV);

#[cfg(unix)]
pub(super) const DAEMON_FAILED_SCENARIO: Scenario = in_daemon(
    "daemon_plan_exit_failed_turn",
    "Discard a daemon session's approval when the proposal turn fails",
    FAILED_SCENARIO,
);

#[cfg(unix)]
pub(super) const DAEMON_QUEUED_CYCLE_SCENARIO: Scenario = in_daemon(
    "daemon_plan_exit_queued_cycle",
    "Apply a daemon session's approved plan mode over a queued Alt+M change",
    QUEUED_CYCLE_SCENARIO,
);
