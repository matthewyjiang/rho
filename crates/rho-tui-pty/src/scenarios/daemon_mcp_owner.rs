//! A daemon session's MCP servers start once, in the daemon.

use anyhow::{ensure, Context, Result};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_mcp_owner",
    "Start a daemon session's MCP server once, in the daemon and not in the TUI as well",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: the TUI that follows a daemon session must not start the
// configured MCP servers itself: the daemon's session owns them, and a
// second launch here runs every server's command twice.
// Owner: interactive TUI as a daemon client.
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    let launches = home.home.join("mcp-launches");
    write_config(&home, &launches)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "1");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        harness.set_phase("session_server_settles");
        harness.wait_for_text("Bypass ·", STARTUP)?;
        harness.submit_text("/mcp")?;
        // The daemon's launch ran and failed, as the command exits.
        harness.wait_for_text("1 problem", STREAM)?;
        harness.inject_key(&crate::keys::Key::Esc)?;

        // A turn runs only once every load of this launch settled.
        harness.set_phase("turn");
        harness.submit_text("hello")?;
        harness.wait_for_text("fixture response: hello", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        let launched = std::fs::read_to_string(&launches)
            .context("read the MCP server's launch record")?
            .lines()
            .count();
        ensure!(
            launched == 1,
            "the MCP server's command ran {launched} times, not once in the daemon"
        );
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

/// A server whose command records each launch in `launches`, then exits.
fn write_config(home: &IsolatedHome, launches: &std::path::Path) -> Result<()> {
    use std::io::Write;

    write_daemon_config(home)?;
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(&home.config_path)?;
    let record = format!("echo launch >> '{}'; exit 1", launches.display());
    writeln!(
        config,
        "\n[mcp.servers.recording]\ntransport = \"stdio\"\ncommand = \"sh\"\nargs = [\"-c\", {record:?}]"
    )?;
    Ok(())
}
