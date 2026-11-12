//! A daemon that restarts in place connects the session's MCP servers
//! again, and the TUI holds a prompt meanwhile as it did at startup.

use std::path::Path;

use anyhow::{ensure, Result};

use super::{daemon_resume::write_daemon_config, fifo, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_restart_mcp",
    "Hold a prompt while a restarted daemon connects the session's MCP servers again",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

/// The idle composer's placeholder; a running turn shows `esc abort`.
const IDLE_COMPOSER: &str = "Type a message";

// Covers: once the daemon restarted in place, the TUI must read again
// whether the session's MCP servers connect. One that still believes
// them connected sends the prompt, and waits on the daemon with the
// keyboard dead until they have.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    let connecting = home.home.join("mcp-connecting");
    write_config(&home, &connecting)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        // A turn saves the session, so the restarted daemon reopens it.
        harness.set_phase("first_connect");
        harness.wait_for_text("Bypass ·", STARTUP)?;
        harness.submit_text("hello before restart")?;
        fifo::release(&mut harness, &connecting)?;
        harness.wait_for_text("fixture response: hello before restart", STREAM)?;
        harness.wait_for_text(IDLE_COMPOSER, SETTLE)?;

        harness.set_phase("restart");
        let restart = home
            .command(&runner.binary)
            .args(["daemon", "restart"])
            .output()?;
        ensure!(
            restart.status.success(),
            "rho daemon restart failed: {}\ndaemon log:\n{}",
            String::from_utf8_lossy(&restart.stderr),
            home.daemon_log()
        );
        harness.wait_for_text("reconnected to rho daemon", STREAM)?;

        // The reopened session's server connects until released.
        harness.set_phase("held_while_reconnecting");
        harness.submit_text("hello after restart")?;
        harness.wait_for_text("HOLD", STREAM)?;

        harness.set_phase("released_once_connected");
        fifo::release(&mut harness, &connecting)?;
        harness.wait_for_text("fixture response: hello after restart", STREAM)?;
        harness.wait_for_text(IDLE_COMPOSER, SETTLE)?;
        let screen = harness.screen().contents();
        ensure!(
            !screen.contains("could not"),
            "the TUI showed an error across the restart:\n{screen}"
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

/// A daemon config with one MCP server that connects until the scenario
/// releases `connecting`, a FIFO this creates, and then fails: what
/// matters is when connect ends, not how.
pub(super) fn write_config(home: &IsolatedHome, connecting: &Path) -> Result<()> {
    use std::io::Write;

    write_daemon_config(home)?;
    fifo::create(connecting)?;
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(&home.config_path)?;
    let wait = format!("read release < '{}'; exit 1", connecting.display());
    writeln!(
        config,
        "\n[mcp.servers.gated]\ntransport = \"stdio\"\ncommand = \"sh\"\nargs = [\"-c\", {wait:?}]"
    )?;
    Ok(())
}
