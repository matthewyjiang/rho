//! Follow another daemon session without leaving the TUI: `/new`,
//! `/resume <id>`, and the `--resume` picker.

use anyhow::{ensure, Result};

use super::{config, daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_switch",
    "Start a new daemon session, resume the first, and pick one at startup",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: `/new` and `/resume` replace the daemon session the TUI follows:
// the screen shows only the followed session's conversation, the next
// prompt runs in it, a new session takes settings at once and keeps those
// of the one before it, the `--resume` picker switches from the session it
// opened over, and a launch flag changes the session it resumes.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix");
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
        harness.set_phase("first_session");
        harness.wait_for_text("Bypass ·", STARTUP)?;
        harness.submit_text("hello alpha")?;
        harness.wait_for_text("fixture response: hello alpha", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        let (first, _) = config::find_latest_session(&home)?;

        // Nothing differs to carry, so the daemon sends nothing after the
        // attach: the new session's options must show without an event.
        harness.set_phase("new_session_takes_settings_at_once");
        harness.submit_text("/new")?;
        harness.wait_for_text_gone("fixture response: hello alpha", STREAM)?;
        harness.submit_text("/permissions supervised")?;
        harness.wait_for_text("Supervised ·", SETTLE)?;

        // A turn's end reports the session's own options, which are the
        // carried ones only if the daemon took them.
        harness.set_phase("new_session_keeps_settings");
        harness.submit_text("/new")?;
        harness.submit_text("hello beta")?;
        harness.wait_for_text("fixture response: hello beta", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        harness.assert_screen_contains("Supervised ·")?;

        harness.set_phase("resume_first_session");
        harness.submit_text(&format!("/resume {first}"))?;
        harness.wait_for_text("fixture response: hello alpha", STREAM)?;
        harness.wait_for_text_gone("fixture response: hello beta", SETTLE)?;
        harness.submit_text("hello gamma")?;
        harness.wait_for_text("fixture response: hello gamma", STREAM)?;
        harness.assert_screen_contains("fixture response: hello alpha")?;
        harness.wait_for_text("Type a message", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the session exited with code {code}");

        harness = spawn(&plan.clone().with_arg("--resume"))?;
        harness.set_phase("startup_picker");
        harness.wait_for_text("Resume session", STARTUP)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("fixture response: hello", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the picked session exited with code {code}");

        harness = spawn(
            &plan
                .clone()
                .with_arg("--resume")
                .with_arg(&first)
                .with_arg("--permission-mode")
                .with_arg("plan"),
        )?;
        harness.set_phase("launch_flag_changes_the_resumed_session");
        harness.wait_for_text("fixture response: hello gamma", STREAM)?;
        harness.wait_for_text("Plan ·", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the flagged session exited with code {code}");
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
