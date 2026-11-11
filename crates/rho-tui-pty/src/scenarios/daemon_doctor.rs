//! `/doctor` reports the daemon session's MCP servers, not the client's.

use std::io::Write;

use anyhow::{ensure, Result};

use super::{config, daemon_resume::write_daemon_config, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

const SIZE: PtySize = PtySize {
    rows: 50,
    cols: 120,
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_doctor_mcp",
    "Report a daemon session's MCP servers in /doctor as in process, not the client's",
    SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: the MCP row of /doctor on a daemon session reads the session's
// servers, and shows the row `doctor_mcp` shows in process for them. The
// client's own config names other servers, so a row built from it differs.
// Owner: interactive TUI as a daemon client (#1685).
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_config(&home, "[mcp.servers.broken]\ntransport = \"stdio\"\ncommand = \"rho-mcp-command-that-does-not-exist\"")?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
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
        // A running turn keeps the session, and its servers, in the daemon.
        // The turn starts once they settle, so the broken one has failed.
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

        write_config(&home, "[mcp.servers.client-only]\nenabled = false\ntransport = \"stdio\"\ncommand = \"false\"")?;
        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&session))?;
        harness.set_phase("doctor_reads_the_session");
        harness.wait_for_text("reply waiting for release", STARTUP)?;
        harness.submit_text("/doctor")?;
        harness.wait_for_text("Authentication", SETTLE)?;
        // The Extensions section sits below the fold.
        harness.inject_key(&Key::End)?;
        harness.wait_for_text(
            "1 server problem, 0 connected, 0 tools; run /mcp for details",
            SETTLE,
        )?;
        ensure!(
            harness.screen().contents().contains("degraded"),
            "MCP row is not degraded"
        );
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text_gone("Extensions", SETTLE)?;
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("Type a message", STREAM)?;
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

fn write_config(home: &IsolatedHome, servers: &str) -> Result<()> {
    write_daemon_config(home)?;
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(&home.config_path)?;
    writeln!(config, "\n{servers}")?;
    Ok(())
}
