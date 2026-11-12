//! A new daemon session exists only once its first prompt is sent.

use anyhow::{ensure, Result};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_lazy_session",
    "Open the daemon session with the first prompt, keeping settings chosen before it",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

/// A daemon-hosted fixture session with two pinned models to cycle through.
fn write_config(home: &IsolatedHome) -> Result<()> {
    std::fs::write(
        &home.config_path,
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
favorite_models = ["openai/gpt-5.5", "xai/grok-4.6"]

[behavior]
permission_mode = "bypass"
credential_store = "file"
"#,
    )?;
    Ok(())
}

// Covers: opening the TUI, or `/new`, and quitting without a prompt leaves
// no session in the daemon's jobs; a model and permission mode chosen
// before the first prompt are the ones the session it opens runs on, and
// that session is listed and resumable afterwards.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "fixture-openai-key")
        .with_env("XAI_API_KEY", "fixture-xai-key");
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
        harness.set_phase("quit_without_prompting");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("/new")?;
        harness.wait_for_text("new session", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the unprompted TUI exited with code {code}");

        // The daemon outlives the TUI; neither run, nor this one, has
        // opened a session in it.
        harness = spawn(&plan)?;
        harness.set_phase("no_jobs_left_behind");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("/jobs")?;
        harness.wait_for_text("No jobs are running", SETTLE)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text_gone("No jobs are running", SETTLE)?;

        harness.set_phase("settings_before_the_first_prompt");
        harness.inject_key(&Key::Ctrl('p'))?;
        harness.wait_for_text("model switched to xai/grok-4.6", SETTLE)?;
        harness.submit_text("/permissions supervised")?;
        harness.wait_for_text("Supervised ·", SETTLE)?;

        // The turn's end reports the session's own options, which are the
        // chosen ones only if the session opened with them.
        harness.set_phase("first_prompt_opens_the_session");
        harness.submit_text("hello lazy")?;
        harness.wait_for_text("fixture response: hello lazy", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        harness.assert_screen_contains("Supervised ·")?;
        harness.assert_screen_contains("grok-4.6")?;
        ensure!(
            !harness.screen().contains_text("gpt-5.5"),
            "the session ran on the launch model:\n{}",
            harness.screen().contents()
        );
        // The one job is the session the prompt opened, on the chosen model.
        harness.submit_text("/jobs")?;
        harness.wait_for_text("▸ ended", SETTLE)?;
        harness.assert_screen_contains("· grok-4.6")?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text_gone("▸ ended", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the prompted session exited with code {code}");
        harness.assert_raw_contains(b"rho --resume ")?;
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
