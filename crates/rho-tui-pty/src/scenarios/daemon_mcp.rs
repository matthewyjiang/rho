//! Inspect the daemon's MCP session while local config and runtime disagree.

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
    "daemon_mcp_inventory",
    "Inspect the daemon's MCP servers during a turn, independently of local config and queued settings",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: /mcp must read the attached daemon session, not the unused local
// runtime built from newer config. A queued configure must not block inventory
// reads or cancellation while a provider turn holds the runtime.
// Owner: interactive TUI as a daemon client.
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_config(&home, "daemon-session-server")?;
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
        harness.set_phase("detach_running_session");
        harness.wait_for_text("Bypass ·", STARTUP)?;
        harness.submit_text("fixture gated reply")?;
        harness.wait_for_text("reply waiting for release", STREAM)?;
        let (session, _) = config::find_latest_session(&home)?;
        harness.submit_text("/exit")?;
        ensure!(
            harness.wait_for_exit(STARTUP)? == 0,
            "initial client failed"
        );

        // The daemon keeps the active session and its original MCP config.
        // Only the next client's unused in-process runtime sees this server.
        write_config(&home, "client-config-server")?;
        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&session))?;
        harness.set_phase("inventory_bypasses_queued_configure");
        harness.wait_for_text("reply waiting for release", STARTUP)?;
        harness.submit_text("/permissions supervised")?;
        harness.wait_for_text("the change applies after the current turn", SETTLE)?;
        harness.submit_text("/mcp")?;
        harness.wait_for_text("daemon-session-server", SETTLE)?;
        ensure!(
            !harness.screen().contents().contains("client-config-server"),
            "inventory came from the local runtime"
        );
        harness.inject_key(&Key::Down)?;
        harness.wait_for_text("enabled = false", SETTLE)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("reply waiting for release", SETTLE)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("Supervised ·", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        ensure!(
            harness.quit_with_exit_command()? == 0,
            "resumed client failed"
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

fn write_config(home: &IsolatedHome, server: &str) -> Result<()> {
    use std::io::Write;

    write_daemon_config(home)?;
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(&home.config_path)?;
    writeln!(
        config,
        "\n[mcp.servers.{server}]\nenabled = false\ntransport = \"stdio\"\ncommand = \"false\""
    )?;
    Ok(())
}
