//! `/login` and `/logout` against a daemon session: the daemon saves and
//! deletes the credential its sessions read.

use anyhow::{ensure, Result};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_login",
    "Save an API key and sign out through the daemon that runs the session",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

/// The onboarding's default base URL, replaced by one nothing listens on.
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000/v1";

// Covers: an API key entered at `/login` is checked and saved by the
// daemon, which says so when the host cannot be asked about it, and
// `/logout` has the daemon delete it.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "1");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        harness.set_phase("custom_host");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("/login")?;
        harness.wait_for_text("Select provider to login", SETTLE)?;
        harness.type_into_field("Chat Completions")?;
        harness.wait_for_text("Custom · Chat Completions", SETTLE)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("edit provider name", SETTLE)?;
        harness.type_into_field("vllm")?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("edit base URL", SETTLE)?;
        // The discard port: the daemon's question about the key gets no
        // answer, whatever else runs on this machine.
        for _ in 0..DEFAULT_BASE_URL.len() {
            harness.inject_key(&Key::Backspace)?;
        }
        harness.type_into_field("http://127.0.0.1:9/v1")?;
        harness.inject_key(&Key::Enter)?;

        harness.set_phase("daemon_saves_the_key");
        harness.wait_for_text("enter API key (optional)", SETTLE)?;
        harness.type_text("sk-local-key")?;
        harness.wait_for_text("••••••••••••", SETTLE)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("saved the vllm key without checking it", STARTUP)?;
        harness.wait_for_text("stored credentials for vllm", STARTUP)?;

        harness.set_phase("daemon_signs_out");
        harness.submit_text("/logout vllm")?;
        harness.wait_for_text("logged out of vllm", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the session exited with code {code}");
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
