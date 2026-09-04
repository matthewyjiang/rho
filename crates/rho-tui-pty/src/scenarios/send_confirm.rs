use super::*;

// Covers: a send whose conversation holds provider-native context the active
// model cannot replay opens a confirm-send modal; Esc returns the prompt and
// "Send anyway" starts the turn.
// Owner: interactive TUI
pub(super) const SEND_CONFIRM_HANDOFF_ID: &str = "send_confirm_handoff";

pub(crate) fn is_send_confirm_scenario(name: &str) -> bool {
    name == SEND_CONFIRM_HANDOFF_ID
}

const SEED_PROMPT: &str = "hello there";

pub(super) fn run_send_confirm_handoff(
    runner: &crate::scenario::ScenarioRunner,
) -> anyhow::Result<crate::scenario::ScenarioOutcome> {
    use crate::{
        artifacts::ArtifactWriter,
        env::{IsolatedHome, RhoLaunchPlan},
        harness::PtyHarness,
        pty::PtySize,
        scenario::ScenarioOutcome,
    };

    const SIZE: PtySize = PtySize {
        rows: 16,
        cols: 100,
    };

    let home = IsolatedHome::new()?;
    let seed_plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix");
    let mut seed = PtyHarness::spawn_named(&seed_plan, "send_confirm_seed")?;
    seed.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        seed.set_artifact_writer(ArtifactWriter::new(root));
    }
    let seed_result = (|| -> anyhow::Result<()> {
        seed.wait_for_text("gpt-5.5", STARTUP)?;
        seed.submit_text(SEED_PROMPT)?;
        seed.wait_for_text(&format!("fixture response: {SEED_PROMPT}"), STREAM)?;
        let code = seed.quit_with_exit_command()?;
        if code != 0 {
            anyhow::bail!("seed session exited with code {code}");
        }
        Ok(())
    })();
    if let Err(error) = seed_result {
        if seed.is_running() {
            let _ = seed.kill();
        }
        return Ok(ScenarioOutcome {
            id: SEND_CONFIRM_HANDOFF_ID.into(),
            passed: false,
            message: format!("seed phase failed: {error:#}"),
            timing: seed.timing().clone(),
            artifact_dir: runner.artifact_root.clone(),
        });
    }

    let (session_id, session_path) = config::find_latest_session(&home)?;
    // Keep stored provider openai so resume stays on gpt-5.5 while assistant
    // history carries anthropic-native blocks the runtime cannot replay.
    config::inject_non_replayable_provider_context(
        &session_path,
        config::StoredProviderRewrite::Keep,
    )?;

    let resume_plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_arg("--resume")
        .with_arg(&session_id);
    let mut harness = PtyHarness::spawn_named(&resume_plan, SEND_CONFIRM_HANDOFF_ID)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> anyhow::Result<()> {
        // The loaded-session handoff opens first; continue with the runtime
        // model so the native blocks stay in history and gate the next send.
        harness.set_phase("loaded_session_handoff_opens");
        harness.wait_for_text("How should Rho continue", STARTUP)?;
        harness.inject_key(&Key::Enter)?;
        // The resume status is a transient toast; the durable signal that the
        // handoff resolved is the composer accepting input again.
        harness.wait_for_text("Type a message", SETTLE)?;

        harness.set_phase("send_opens_confirm_modal");
        harness.submit_text(SEED_PROMPT)?;
        harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
        harness.assert_screen_contains("Send anyway")?;
        harness.assert_screen_contains("Don't send")?;

        harness.set_phase("esc_returns_prompt");
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("send cancelled", SETTLE)?;
        harness.assert_screen_contains(SEED_PROMPT)?;

        harness.set_phase("send_anyway_starts_turn");
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
        harness.inject_key(&Key::Char('1'))?;
        harness.wait_for_text(&format!("fixture response: {SEED_PROMPT}"), STREAM)?;
        let code = harness.quit_with_exit_command()?;
        if code != 0 {
            anyhow::bail!("session exited with code {code}");
        }
        Ok(())
    })();

    Ok(match result {
        Ok(()) => ScenarioOutcome {
            id: SEND_CONFIRM_HANDOFF_ID.into(),
            passed: true,
            message: "ok".into(),
            timing: harness.timing().clone(),
            artifact_dir: None,
        },
        Err(error) => {
            if harness.is_running() {
                let _ = harness.kill();
            }
            ScenarioOutcome {
                id: SEND_CONFIRM_HANDOFF_ID.into(),
                passed: false,
                message: format!("{error:#}"),
                timing: harness.timing().clone(),
                artifact_dir: runner.artifact_root.clone(),
            }
        }
    })
}
