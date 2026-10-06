//! Project onboarding targets the git root, then session entry reloads instructions.

use std::path::Path;

use anyhow::{ensure, Context, Result};

use super::{
    assert_helpers::{saved_session_with_text, wait_for_turn_completion_after},
    DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    scenario::{Scenario, Step},
    IsolatedHome, PtyHarness,
};

const INSTRUCTIONS: &str =
    "# Project instructions\n\nUse fixture-init-project-rule when working in this repository.\n";
const UNRELATED: &str = "# Existing guidance\n\nKeep unrelated-fixture-guidance intact.\n";

const INIT_STEPS: &[Step] = &[
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/init"),
    Step::WaitText {
        text: "fixture init wrote AGENTS.md",
        timeout: STREAM,
    },
    Step::Custom(assert_created_and_cached),
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
    Step::Custom(assert_resume_reloads_instructions),
    Step::ExitCommand,
];

// Covers: /init must execute its initial skill and write at the git root, not
// nested cwd; /new loads the new file and /resume re-reads subsequent edits.
// Owner: interactive TUI onboarding and session lifecycle.
pub(super) const INIT_COMMAND_SCENARIO: Scenario = Scenario::new(
    "init_command",
    "Create root instructions from nested cwd and reload on /new and /resume",
    DEFAULT_SIZE,
    INIT_STEPS,
    /*smoke*/ false,
)
.with_setup(setup_nested_repository);

// Covers: existing root instructions must be edited without losing unrelated
// guidance; /new must load the addition rather than reuse the old snapshot.
// Owner: interactive TUI onboarding and session lifecycle.
pub(super) const INIT_EXISTING_SCENARIO: Scenario = Scenario::new(
    "init_existing",
    "Update existing root instructions from nested cwd without replacing guidance",
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
        Step::Custom(assert_updated_and_cached),
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
)
.with_setup(setup_existing_instructions);

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
.with_setup(setup_nested_repository)
.with_args(&["--permission-mode", "plan"]);

fn setup_nested_repository(home: &IsolatedHome) -> Result<()> {
    // The harness launches in workspace/, beneath this distinct git root.
    // Project discovery only needs the repository marker, not a git subprocess.
    std::fs::create_dir(home.path().join(".git"))?;
    Ok(())
}

fn setup_existing_instructions(home: &IsolatedHome) -> Result<()> {
    setup_nested_repository(home)?;
    std::fs::write(home.path().join("AGENTS.md"), UNRELATED)?;
    Ok(())
}

fn assert_created_and_cached(harness: &mut PtyHarness) -> Result<()> {
    assert_written_and_cached(harness, INSTRUCTIONS)
}

fn assert_updated_and_cached(harness: &mut PtyHarness) -> Result<()> {
    assert_written_and_cached(harness, &format!("{UNRELATED}\n{INSTRUCTIONS}"))
}

fn assert_written_and_cached(harness: &mut PtyHarness, expected: &str) -> Result<()> {
    let cwd = harness.working_directory().context("missing workspace")?;
    let root = cwd.parent().context("missing git root")?;
    ensure!(
        std::fs::read_to_string(root.join("AGENTS.md"))? == expected,
        "/init did not preserve the complete root instruction content"
    );
    assert_nested_instructions_absent(cwd)?;
    wait_for_turn_completion_after(harness, "fixture init wrote AGENTS.md")?;
    harness.submit_text("fixture init instructions")?;
    harness.wait_for_text("fixture init instructions missing", STREAM)?;
    wait_for_turn_completion_after(harness, "fixture init instructions missing")
}

// Change disk contents after /new, then return to the original saved session.
// Unique probe phases avoid matching responses already present in restored history.
fn assert_resume_reloads_instructions(harness: &mut PtyHarness) -> Result<()> {
    wait_for_turn_completion_after(harness, "fixture init instructions loaded")?;
    let root = harness
        .working_directory()
        .and_then(Path::parent)
        .context("missing git root")?
        .to_path_buf();
    let id = saved_session_with_text(harness, "fixture init wrote AGENTS.md")?;
    std::fs::remove_file(root.join("AGENTS.md"))?;
    harness.submit_text("fixture init instructions before resume")?;
    harness.wait_for_text("fixture init instructions loaded before resume", STREAM)?;
    wait_for_turn_completion_after(harness, "fixture init instructions loaded before resume")?;
    harness.submit_text(&format!("/resume {id}"))?;
    harness.wait_for_text("fixture init instructions missing", SETTLE)?;
    harness.submit_text("fixture init instructions after resume")?;
    harness.wait_for_text("fixture init instructions missing after resume", STREAM)?;
    wait_for_turn_completion_after(harness, "fixture init instructions missing after resume")
}

fn assert_nested_instructions_absent(cwd: &Path) -> Result<()> {
    ensure!(
        !cwd.join("AGENTS.md").try_exists()?,
        "/init wrote instructions at nested cwd instead of the git root"
    );
    Ok(())
}

fn assert_no_instructions(harness: &mut PtyHarness) -> Result<()> {
    let cwd = harness.working_directory().context("missing workspace")?;
    let root = cwd.parent().context("missing git root")?;
    ensure!(
        !root.join("AGENTS.md").try_exists()?,
        "Plan onboarding wrote root instructions"
    );
    assert_nested_instructions_absent(cwd)
}
