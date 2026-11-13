//! Quit a daemon-hosted turn without cancelling it, then resume its reply.

use std::time::{Duration, Instant};

use anyhow::{ensure, Result};

use super::{config, daemon_resume::write_daemon_config, DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
    screen::ScreenModel,
};

/// How long the turn runs before the quit: far longer than a frame takes to
/// reach the screen, so a clock that restarted on reopen cannot catch up.
const RUN_BEFORE_QUIT: Duration = Duration::from_secs(3);

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_detach",
    "Detach a running daemon turn and resume it using the new session's exit hint",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

// Covers: quitting must not cancel a daemon turn, and reopening it mid-reply
// shows it responding, its clock counting from the turn's start; a newly
// saved session's receipt must identify the session the user can resume,
// while an untouched session must not advertise a nonexistent saved
// conversation.
// Owner: interactive daemon session lifecycle
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
        harness.wait_for_screen(
            &format!("turn clock past {RUN_BEFORE_QUIT:?}"),
            SETTLE,
            |screen| turn_clock(screen).is_some_and(|clock| clock >= RUN_BEFORE_QUIT),
        )?;
        let (session_id, _) = config::find_latest_session(&home)?;
        // Unlike quit_with_exit_command, do not send Esc: it cancels a turn.
        harness.submit_text("/exit")?;
        let code = harness.wait_for_exit(STARTUP)?;
        ensure!(code == 0, "running session exited with code {code}");
        let short_id: String = session_id.chars().take(8).collect();
        harness.assert_raw_contains(format!("rho --resume {short_id}").as_bytes())?;

        let quit_at = Instant::now();

        // Reopened mid-reply, the turn shows busy and responding, as it did
        // before the quit, and quitting again leaves it running. Its clock
        // keeps the time before the quit: one counting from the reopen
        // always reads less than the time since the quit.
        harness = spawn(&plan.clone().with_arg("--resume").with_arg(&short_id))?;
        harness.set_phase("reopen_running_turn");
        harness.wait_for_text("reply waiting for release", STREAM)?;
        harness.wait_for_screen(
            "responding, with the turn's clock from before the quit",
            SETTLE,
            |screen| turn_clock(screen).is_some_and(|clock| clock >= quit_at.elapsed()),
        )?;
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

/// The running turn's clock, as its `responding · 4.2s` activity row shows it.
fn turn_clock(screen: &ScreenModel) -> Option<Duration> {
    screen.rows_text().iter().find_map(|row| {
        let (_, rest) = row.split_once("responding · ")?;
        let seconds = rest.split_once('s')?.0.trim();
        seconds.parse().ok().map(Duration::from_secs_f64)
    })
}
