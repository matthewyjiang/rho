//! Saved desktop consent across real process restarts, using only a fake driver.

use anyhow::{ensure, Result};

use super::{computer::setup_driver, config::find_latest_session, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
    timing::TimingSummary,
};

pub(super) const COMPUTER_PREFERENCE_SCENARIO: Scenario = Scenario::new(
    "computer_preference",
    "Session consent survives resume independently of new-session defaults and startup restrictions",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[],
    /*smoke*/ false,
);

pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    setup_driver(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, COMPUTER_PREFERENCE_SCENARIO.size)
        .with_env("PATH", "")
        .with_env("DISPLAY", "");
    let mut timing = TimingSummary::default();
    let result = (|| -> Result<()> {
        run_phase(runner, &plan, "grant", &mut timing, |harness| {
            harness.submit_text("/computer")?;
            harness.wait_for_text("No desktop access granted", SETTLE)?;
            harness.inject_key(&Key::Esc)?;
            harness.submit_text("/computer on")?;
            harness.wait_for_text("Grant desktop access?", SETTLE)?;
            harness.inject_key(&Key::Char('g'))?;
            harness.wait_for_text("computer use enabled", STARTUP)?;
            harness.submit_text("computer preference session A")?;
            harness.wait_for_text("fixture response: computer preference session A", STREAM)
        })?;
        let (session_a, _) = find_latest_session(&home)?;
        // The saved grant must not escape the native interactive tool boundary.
        // These fresh processes reuse the exact home written by the grant above.
        for (name, args) in [
            ("plan", &["--permission-mode", "plan"][..]),
            ("no_tools", &["--no-tools"][..]),
        ] {
            let restricted = args
                .iter()
                .fold(plan.clone(), |plan, arg| plan.with_arg(*arg));
            run_phase(runner, &restricted, name, &mut timing, |harness| {
                harness.submit_text("fixture tool available computer")?;
                harness.wait_for_text("tool available computer: false", STREAM)?;
                harness.submit_text("/computer")?;
                harness.wait_for_text("No desktop access granted", SETTLE)?;
                harness.inject_key(&Key::Esc)
            })?;
        }
        run_phase(runner, &plan, "restored", &mut timing, |harness| {
            // No command or confirmation before this wait: startup owns restoration.
            harness.wait_for_text("computer use enabled", STARTUP)?;
            harness.submit_text("fixture computer context")?;
            harness.wait_for_text("computer context: enabled", STREAM)?;
            harness.submit_text("fixture delay")?;
            harness.wait_for_text("partial assistant before cancellation", STREAM)?;
            harness.submit_text("/computer off")?;
            harness.inject_key(&Key::Esc)?;
            harness.wait_for_text("model interrupted", STREAM)?;
            harness.submit_text("fixture tool available computer")?;
            harness.wait_for_text("tool available computer: false", STREAM)
        })?;
        let (session_b, _) = find_latest_session(&home)?;
        ensure!(
            session_a != session_b,
            "expected two distinct saved sessions"
        );
        run_phase(runner, &plan, "disabled", &mut timing, |harness| {
            harness.submit_text("fixture tool available computer")?;
            harness.wait_for_text("tool available computer: false", STREAM)?;
            harness.submit_text("fixture computer context")?;
            harness.wait_for_text("computer context: disabled", STREAM)?;
            harness.submit_text("/computer")?;
            harness.wait_for_text("No desktop access granted", SETTLE)?;
            harness.inject_key(&Key::Esc)
        })?;
        // Covers: resuming an existing session must not borrow or overwrite the
        // new-session default. Owner: interactive session lifecycle across processes.
        let resume_a = plan.clone().with_arg("--resume").with_arg(&session_a);
        run_phase(runner, &resume_a, "resume_a", &mut timing, |harness| {
            harness.wait_for_text("computer use enabled", STARTUP)?;
            harness.submit_text("fixture tool available computer")?;
            harness.wait_for_text("tool available computer: true", STREAM)?;
            // Resuming A must leave the last explicit choice (B's off) intact.
            harness.submit_text("/new")?;
            harness.wait_for_text_gone("tool available computer: true", SETTLE)?;
            harness.submit_text("fixture computer context")?;
            harness.wait_for_text("computer context: disabled", STREAM)?;
            harness.submit_text("/computer on")?;
            harness.wait_for_text("Grant desktop access?", SETTLE)?;
            harness.inject_key(&Key::Char('g'))?;
            harness.wait_for_text("computer use enabled", STARTUP)
        })?;
        let resume_b = plan.clone().with_arg("--resume").with_arg(&session_b);
        run_phase(runner, &resume_b, "resume_b", &mut timing, |harness| {
            // B's old history has enabled context, so this cannot match replay.
            harness.submit_text("fixture computer context")?;
            harness.wait_for_text("computer context: disabled", STREAM)?;
            harness.submit_text("/new")?;
            harness.wait_for_text_gone("computer context: disabled", SETTLE)?;
            harness.wait_for_text("computer use enabled", STARTUP)?;
            harness.submit_text("fixture tool available computer")?;
            harness.wait_for_text("tool available computer: true", STREAM)?;
            // The command path must also restore the target's own saved choice.
            harness.submit_text(&format!("/resume {session_b}"))?;
            harness.wait_for_text("resumed session", STARTUP)?;
            harness.submit_text("/computer")?;
            harness.wait_for_text("No desktop access granted", SETTLE)?;
            harness.inject_key(&Key::Esc)
        })
    })();
    Ok(ScenarioOutcome {
        id: COMPUTER_PREFERENCE_SCENARIO.id.into(),
        passed: result.is_ok(),
        message: result
            .as_ref()
            .map_or_else(|error| format!("{error:#}"), |()| "ok".into()),
        timing,
        artifact_dir: result.err().and(runner.artifact_root.clone()),
    })
}

fn run_phase(
    runner: &ScenarioRunner,
    plan: &RhoLaunchPlan,
    name: &str,
    timing: &mut TimingSummary,
    steps: impl FnOnce(&mut PtyHarness) -> Result<()>,
) -> Result<()> {
    let mut harness = PtyHarness::spawn_named(plan, format!("computer_preference_{name}"))?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        steps(&mut harness)?;
        ensure!(
            harness.quit_with_exit_command()? == 0,
            "computer preference {name} did not exit cleanly"
        );
        Ok(())
    })();
    timing.samples.extend(harness.timing().samples.clone());
    if result.is_err() && harness.is_running() {
        let _ = harness.kill();
    }
    result
}
