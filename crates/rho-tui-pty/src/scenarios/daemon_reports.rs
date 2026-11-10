//! Commands that ask a daemon session about itself or widen its scope:
//! `/add-dir`, `/context`, and `/hooks`.

use anyhow::{ensure, Context, Result};

use super::{config, daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_session_reports",
    "Add a directory to a daemon session and read its context and hooks reports",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: `/add-dir` widens the daemon's session, not only this screen: a
// TUI that reopens the session lists the directory; `/context` and `/hooks`
// open on what the daemon reports for the session it owns.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let extra = home
        .workspace
        .parent()
        .context("the workspace has a parent")?
        .join("shared-notes");
    std::fs::create_dir(&extra)?;
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
        harness.set_phase("add_directory");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("hello first")?;
        harness.wait_for_text("fixture response: hello first", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        harness.submit_text(&format!("/add-dir {}", extra.display()))?;
        harness.wait_for_text("added directory", SETTLE)?;

        harness.set_phase("context_report");
        harness.submit_text("/context")?;
        harness.wait_for_text("System prompt", SETTLE)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("Type a message", SETTLE)?;

        harness.set_phase("hooks_report");
        harness.submit_text("/hooks")?;
        harness.wait_for_text("no hooks files found", SETTLE)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        let (session_id, _) = config::find_latest_session(&home)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the session exited with code {code}");

        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&session_id))?;
        harness.set_phase("resume_keeps_the_directory");
        harness.wait_for_text("fixture response: hello first", STREAM)?;
        harness.submit_text("/add-dir")?;
        // The replayed row names the directory too: the listing must. A
        // long path wraps the usage that follows it, so wait for the
        // listing's start.
        harness.wait_for_text("added directories:", SETTLE)?;
        ensure!(
            !harness.screen().contains_text("added directories: none"),
            "the reopened session lists no added directory"
        );
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
