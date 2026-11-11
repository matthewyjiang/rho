//! The workflow hub and rail, and `/codemode`, in a daemon session.

use anyhow::{ensure, Context, Result};

use super::{
    config,
    daemon_resume::write_daemon_config,
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
    "daemon_workflow",
    "Start a workflow from the hub in a daemon session, keep it running across a TUI restart, and set codemode",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: the hub's Start row in a daemon session starts the run in the
// daemon, not in this process, with no row for the prompt the hub sent,
// live or when the session is reopened:
// the workflow rail shows its progress, the run is still going on the rail
// of a TUI that reopens the session after the first one quit, and its
// result arrives once released. `/codemode only`
// saves the mode through the daemon, where `/codemode` then reads it.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    setup_workflow(&home)?;
    let runs = home.rho_home().join("workflows/runs");
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
        harness.set_phase("hub_start");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("/workflow")?;
        harness.wait_for_text("Workflows", STREAM)?;
        // The only source is the first hub row.
        harness.inject_key(&Key::Enter)?;
        assert_live_rail(&mut harness)?;
        // The prompt the hub sent for the start is the hub's spelling and
        // has no row, as in process: the start's notice stands alone.
        assert_hub_prompt_hidden(&mut harness)?;
        let started = record_ids(&runs)?;
        ensure!(started.len() == 1, "the hub started one run: {started:?}");

        harness.set_phase("run_outlives_the_tui");
        let (session_id, _) = config::find_latest_session(&home)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the session exited with code {code}");
        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&session_id))?;
        assert_live_rail(&mut harness)?;
        // The daemon kept no user message for it, so a reopened session
        // replays none.
        assert_hub_prompt_hidden(&mut harness)?;
        release_gate(&mut harness)?;
        // The daemon gives the idle session the result in a turn of its own.
        harness.wait_for_text("(pty-background) finished - success", STREAM)?;
        ensure!(
            record_ids(&runs)? == started,
            "the reopened session started no second run"
        );

        harness.set_phase("codemode");
        harness.submit_text("/codemode only")?;
        harness.wait_for_text("codemode: Only; applies next turn", SETTLE)?;
        // `/codemode` reads the settings the daemon publishes once it
        // saved them.
        let deadline = std::time::Instant::now() + SETTLE.duration;
        while !std::fs::read_to_string(&home.config_path)
            .context("read the config")?
            .contains("mode = \"only\"")
        {
            ensure!(
                std::time::Instant::now() < deadline,
                "the daemon did not save the mode"
            );
            harness.pump(std::time::Duration::from_millis(25));
        }
        harness.submit_text("/codemode")?;
        harness.wait_for_text("codemode only: tools use codemode", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the resumed session exited with code {code}");
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

/// Waits for the hub start's notice and checks that no row shows the
/// `/workflow:` prompt the hub sent for it.
fn assert_hub_prompt_hidden(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("Starting 'pty-background'", STREAM)?;
    ensure!(
        !harness.screen().contains_text("/workflow:pty-background"),
        "the hub's start shows its prompt:\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}
