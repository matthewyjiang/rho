//! A workflow the hub starts while a daemon session's MCP servers connect
//! is held like a prompt, and starts once they have.

use anyhow::{ensure, Result};

use super::{
    daemon_restart_mcp::write_config,
    fifo,
    workflow_background::{assert_live_rail, record_ids, release_gate, setup_workflow},
    DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_workflow_hold",
    "Hold a hub workflow start during a daemon session's MCP connect, cancel one with Esc, and start the other once connected",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: the hub's Start row during MCP connect must not send the launch
// and wait on the daemon with the keyboard dead. It is held, Esc cancels
// it, and one still held starts by itself once the servers connected.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    let connecting = home.home.join("mcp-connecting");
    write_config(&home, &connecting)?;
    setup_workflow(&home)?;
    let runs = home.rho_home().join("workflows/runs");
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "1");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    // The only source is the first hub row.
    let start_from_hub = |harness: &mut PtyHarness| -> Result<()> {
        harness.submit_text("/workflow")?;
        harness.wait_for_text("Workflows", STREAM)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("HOLD", STREAM)
    };
    let result = (|| -> Result<()> {
        harness.set_phase("held_start_cancelled");
        harness.wait_for_text("Bypass ·", STARTUP)?;
        start_from_hub(&mut harness)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text_gone("HOLD", SETTLE)?;

        harness.set_phase("held_start_runs_once_connected");
        start_from_hub(&mut harness)?;
        fifo::release(&mut harness, &connecting)?;
        assert_live_rail(&mut harness)?;
        // The cancelled start never reached the daemon.
        let started = record_ids(&runs)?;
        ensure!(started.len() == 1, "the hub started one run: {started:?}");

        release_gate(&mut harness)?;
        harness.wait_for_text("✓ done · 3/3 tasks", STREAM)?;
        ensure!(
            harness.quit_with_exit_command()? == 0,
            "the TUI exited with an error"
        );
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
