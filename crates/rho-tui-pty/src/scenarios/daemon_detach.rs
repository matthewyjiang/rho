//! Quit a daemon-hosted turn without cancelling it, then resume its reply.

use anyhow::{ensure, Result};

use super::{config, daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_detach",
    "Detach a running daemon turn and resume it using the new session's exit hint",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: quitting must not cancel a daemon turn, and reopening it mid-reply
// shows it responding; a newly saved session's receipt must identify the
// session the user can resume, while an untouched session must not
// advertise a nonexistent saved conversation.
// Owner: interactive daemon session lifecycle
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
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
        harness.set_phase("quit_untouched_session");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "untouched session exited with code {code}");
        ensure!(
            !String::from_utf8_lossy(harness.raw_output()).contains("rho --resume "),
            "an untouched session advertised a resume hint"
        );

        harness = spawn(&plan)?;
        harness.set_phase("detach_running_turn");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text("fixture gated reply")?;
        harness.wait_for_text("reply waiting for release", STREAM)?;
        let (session_id, _) = config::find_latest_session(&home)?;
        // Unlike quit_with_exit_command, do not send Esc: it cancels a turn.
        harness.submit_text("/exit")?;
        let code = harness.wait_for_exit(STARTUP)?;
        ensure!(code == 0, "running session exited with code {code}");
        let short_id: String = session_id.chars().take(8).collect();
        harness.assert_raw_contains(format!("rho --resume {short_id}").as_bytes())?;

        // Reopened mid-reply, the turn shows busy and responding, as it did
        // before the quit, and quitting again leaves it running.
        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&short_id))?;
        harness.set_phase("reopen_running_turn");
        harness.wait_for_text("reply waiting for release", STREAM)?;
        harness.wait_for_text("responding ·", SETTLE)?;
        harness.submit_text("/exit")?;
        let code = harness.wait_for_exit(STARTUP)?;
        ensure!(
            code == 0,
            "reopened running session exited with code {code}"
        );

        // Release only after the TUIs exited. Cancellation on quit would
        // leave no completed reply for the resumed client to see.
        super::fixture_release::release_fixture(&mut harness, ".rho-fixture-release-reply")?;
        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&short_id))?;
        harness.set_phase("resume_detached_turn");
        harness.wait_for_text("reply completed after release", STREAM)?;
        harness.wait_for_text("Type a message", STREAM)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "resumed session exited with code {code}");
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
