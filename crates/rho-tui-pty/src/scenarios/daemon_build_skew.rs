//! The daemon-hosted TUI says at launch that its daemon is another build
//! (#1776), with what restarting the daemon would interrupt.

use anyhow::{ensure, Result};

use super::{daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_build_skew",
    "Show at launch that the daemon is another build, with the work a restart interrupts",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: a TUI whose daemon is another build of the same protocol version
// learns it at launch, before any request can fail on it, with what
// running `rho daemon restart` would interrupt.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    // Starts the sandbox's daemon as another build, which keeps the gated
    // turn in progress until the sandbox stops it.
    let detached = home
        .command(&runner.binary)
        .args(["run", "--detach", "fixture gated reply"])
        .env("RHO_TUI_TEST_MODE", "matrix")
        .env("RHO_TUI_FIXTURE_WORKSPACE", &home.workspace)
        .env("OPENAI_API_KEY", "sk-test-matrix")
        .env("RHO_TUI_TEST_BUILD_ID", "another build")
        .output()?;
    ensure!(
        detached.status.success(),
        "rho run --detach failed: {}\ndaemon log:\n{}",
        String::from_utf8_lossy(&detached.stderr),
        home.daemon_log()
    );
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "1");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        harness.set_phase("launch");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        // The notice wraps, so it is matched across rows. The other clients
        // it counts depend on whether the daemon noticed the detached run
        // leave, so only the work it counts is matched.
        harness.wait_for_screen(
            "the build notice with the detached turn",
            SETTLE,
            |screen| {
                let rows = screen.rows_text();
                let text = rows.iter().map(|row| row.trim()).collect::<Vec<_>>();
                let text = text.join(" ");
                text.contains("rho daemon runs a different build than this client")
                    && text.contains("which interrupts 1 in progress")
            },
        )?;
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
