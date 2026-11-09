//! A daemon-owned goal outlives the TUI that set it.

use anyhow::{ensure, Result};

use super::{config, daemon_resume::write_daemon_config, DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_goal",
    "Block a daemon goal, quit, then inspect and resume it from a new TUI",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: the goal lives in the daemon session, not the TUI: a TUI that
// reopens the session sees the blocked goal its predecessor set and resumes
// it to completion.
// Owner: daemon goal controller and the TUI as its client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "1");
    let spawn = |plan: &RhoLaunchPlan| -> Result<PtyHarness> {
        let mut harness = PtyHarness::spawn_named(plan, SCENARIO.id)?;
        harness.enable_timing(runner.record_timing);
        if let Some(root) = &runner.artifact_root {
            harness.set_artifact_writer(ArtifactWriter::new(root));
        }
        Ok(harness)
    };
    let mut harness = spawn(&plan)?;
    let result = (|| -> Result<()> {
        harness.set_phase("block_goal");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("/goal fixture goal blocked")?;
        harness.wait_for_text("goal blocked: remaining steps need you", STREAM)?;
        let (session_id, _) = config::find_latest_session(&home)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the first TUI exited with code {code}");

        let short_id: String = session_id.chars().take(8).collect();
        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&short_id))?;
        harness.set_phase("inspect_goal_after_reopen");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("/goal")?;
        harness.wait_for_text("goal blocked: fixture goal blocked", STREAM)?;
        harness.wait_for_text("publish the fixture release", STREAM)?;

        harness.set_phase("resume_goal");
        harness.submit_text("/goal resume")?;
        harness.wait_for_text("goal achieved", STREAM)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the reopened TUI exited with code {code}");
        Ok(())
    })();

    if result.is_err() && harness.is_running() {
        let _ = harness.kill();
    }
    Ok(ScenarioOutcome {
        id: SCENARIO.id.into(),
        passed: result.is_ok(),
        message: result
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default(),
        timing: harness.timing().clone(),
        artifact_dir: runner.artifact_root.clone(),
    })
}
