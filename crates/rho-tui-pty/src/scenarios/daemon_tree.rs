//! `/tree` in a daemon session: the daemon moves the conversation to the
//! node chosen and every client replays it from there.

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
    "daemon_tree",
    "Restore an earlier turn of a daemon session from /tree and continue from it",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: `/tree` lists the daemon session's turns from its file; restoring
// the first one makes the daemon continue from it, so the screen replays
// the conversation without the second turn, the next prompt is answered on
// that branch, and a TUI that reopens the session shows the branch too.
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
        harness.set_phase("two_turns");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        for prompt in ["hello first", "hello second"] {
            harness.submit_text(prompt)?;
            harness.wait_for_text(&format!("fixture response: {prompt}"), STREAM)?;
            harness.wait_for_text("Type a message", SETTLE)?;
        }

        harness.set_phase("restore_the_first_turn");
        harness.submit_text("/tree")?;
        harness.wait_for_text("Conversation tree", SETTLE)?;
        // The picker opens on the active turn, the second.
        harness.inject_key(&Key::Up)?;
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text_gone("fixture response: hello second", STREAM)?;
        harness.wait_for_text("fixture response: hello first", STREAM)?;

        harness.set_phase("continue_from_it");
        harness.submit_text("hello third")?;
        harness.wait_for_text("fixture response: hello third", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        // Settled by now: the replay that followed the restore is done.
        ensure!(
            !harness.screen().contains_text("hello second"),
            "the restored conversation still shows the turn left behind:\n{}",
            harness.screen().debug_dump()
        );
        let (session_id, _) = config::find_latest_session(&home)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the session exited with code {code}");

        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&session_id))?;
        harness.set_phase("resume_shows_the_branch");
        harness.wait_for_text("fixture response: hello third", STREAM)?;
        let screen = harness.screen();
        ensure!(
            screen.contains_text("fixture response: hello first")
                && !screen.contains_text("hello second"),
            "the reopened session replays the branch it continued on:\n{}",
            screen.debug_dump()
        );
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
