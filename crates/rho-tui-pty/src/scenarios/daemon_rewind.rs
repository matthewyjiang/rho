//! `/rewind` in a daemon session: the daemon's turns take workspace
//! checkpoints, and the daemon restores one and the conversation before it.

use anyhow::{ensure, Result};

use super::{
    assert_helpers::wait_for_turn_completion_after, daemon_resume::write_daemon_config,
    DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

/// The turn the scenario rewinds.
const PROMPT: &str = "fixture tool";

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_rewind",
    "Rewind the file a daemon session's turn wrote and the conversation with it",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: a turn the daemon runs takes a workspace checkpoint; `/rewind`
// previews it from the session's files, and confirming has the daemon
// delete the file the turn wrote and return the conversation to before the
// turn, which the screen replays and the next prompt continues from.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix");
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
        let written = home.workspace.join(".rho-tui-fixture-output.txt");
        harness.set_phase("a_turn_writes_a_file");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text(PROMPT)?;
        wait_for_turn_completion_after(&mut harness, "tool lifecycle complete with one result")?;
        ensure!(written.exists(), "the turn wrote no file");

        harness.set_phase("preview_and_confirm");
        harness.submit_text("/rewind")?;
        harness.wait_for_text("Workspace rewind", SETTLE)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("delete  .rho-tui-fixture-output.txt", SETTLE)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text(
            "workspace rewind audit; conversation restored to before the turn",
            STREAM,
        )?;
        ensure!(!written.exists(), "the rewind left the turn's file");

        harness.set_phase("continue_from_before_the_turn");
        harness.submit_text("hello after")?;
        harness.wait_for_text("fixture response: hello after", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        // Settled by now: the replay that followed the rewind is done.
        // The turn's prompt goes with its answer: the conversation is the
        // one from before the prompt was saved.
        ensure!(
            !harness.screen().contains_text("tool lifecycle complete")
                && !harness.screen().contains_text(PROMPT),
            "the conversation still shows the rewound turn:\n{}",
            harness.screen().debug_dump()
        );
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
