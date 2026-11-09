//! The daemon-hosted TUI keeps following its session across a daemon that
//! re-executes in place (#1713), as an upgrade or `rho daemon restart`
//! does.

use anyhow::{ensure, Result};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_restart",
    "Restart the daemon under the TUI, which reconnects and takes the next prompt",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

/// The idle composer's placeholder; a running turn shows `esc abort`.
const IDLE_COMPOSER: &str = "Type a message";

// Covers: when the daemon restarts in place, the TUI attached to it
// reconnects without an error, keeps the conversation, and its next prompt
// runs in the restarted daemon; a TUI whose session had saved nothing yet,
// which the restarted daemon cannot reopen, goes on in a new session.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "1");
    let spawn = || -> Result<PtyHarness> {
        let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
        harness.enable_timing(runner.record_timing);
        if let Some(root) = &runner.artifact_root {
            harness.set_artifact_writer(ArtifactWriter::new(root));
        }
        Ok(harness)
    };
    let restart = || -> Result<()> {
        let restart = home
            .command(&runner.binary)
            .args(["daemon", "restart"])
            .output()?;
        ensure!(
            restart.status.success(),
            "rho daemon restart failed: {}",
            String::from_utf8_lossy(&restart.stderr)
        );
        Ok(())
    };
    let mut harness = spawn()?;
    let result = (|| -> Result<()> {
        harness.set_phase("prompt");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("hello before restart")?;
        harness.wait_for_text("fixture response: hello before restart", STREAM)?;
        harness.wait_for_text(IDLE_COMPOSER, SETTLE)?;

        harness.set_phase("restart");
        restart()?;
        harness.wait_for_text("reconnected to rho daemon", STREAM)?;

        harness.set_phase("prompt_again");
        harness.submit_text("hello after restart")?;
        harness.wait_for_text("fixture response: hello after restart", STREAM)?;
        harness.wait_for_text(IDLE_COMPOSER, SETTLE)?;
        let screen = harness.screen().contents();
        ensure!(
            screen.contains("fixture response: hello before restart"),
            "the reconnected TUI lost the conversation before the restart"
        );
        ensure!(
            !screen.contains("could not"),
            "the TUI showed an error across the restart:\n{screen}"
        );
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the TUI exited with code {code}");

        harness = spawn()?;
        harness.set_phase("restart_unsaved");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        restart()?;
        harness.wait_for_text("(new session)", STREAM)?;
        harness.submit_text("hello in a new session")?;
        harness.wait_for_text("fixture response: hello in a new session", STREAM)?;
        harness.wait_for_text(IDLE_COMPOSER, SETTLE)?;
        let screen = harness.screen().contents();
        ensure!(
            !screen.contains("could not"),
            "the TUI showed an error across the restart:\n{screen}"
        );
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the TUI exited with code {code}");
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
