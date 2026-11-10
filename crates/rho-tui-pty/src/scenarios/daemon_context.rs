//! Commands that add to a daemon session without a model turn: `/remember`,
//! `/title`, and a `!` shell command's output.

use anyhow::{ensure, Result};

use super::{config, daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_session_context",
    "Remember an instruction, title the session, and include shell output in a daemon session",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: `/remember` and `!` output reach the daemon's session, not only
// this screen: a TUI that reopens the session replays both rows, and
// `/title` renames the session the daemon holds.
// Owner: interactive TUI as a daemon client
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
        harness.set_phase("add_context");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("hello first")?;
        harness.wait_for_text("fixture response: hello first", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        harness.submit_text("/remember keep replies short")?;
        harness.wait_for_text("remembered in", SETTLE)?;
        let instructions = std::fs::read_to_string(home.workspace.join("AGENTS.md"))?;
        ensure!(
            instructions.contains("keep replies short"),
            "/remember did not write the instruction: {instructions:?}"
        );
        harness.submit_text("/title Night shift")?;
        harness.wait_for_text("session titled: Night shift", SETTLE)?;
        harness.submit_text("!echo shell-marker-7")?;
        harness.wait_for_text("shell output pending context insertion", STREAM)?;
        // Prompts queue behind the context changes, so this reply means the
        // daemon took both.
        harness.submit_text("hello second")?;
        harness.wait_for_text("fixture response: hello second", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        let (session_id, _) = config::find_latest_session(&home)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the session exited with code {code}");

        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&session_id))?;
        harness.set_phase("resume_shows_added_context");
        harness.wait_for_text("fixture response: hello second", STREAM)?;
        harness.assert_screen_contains("remembered in")?;
        harness.assert_screen_contains("!echo shell-marker-7")?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the resumed session exited with code {code}");
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
