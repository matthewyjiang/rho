use super::*;

// Covers: a send whose conversation holds provider-native context the active
// model cannot replay opens a confirm-send modal; Esc returns the prompt and
// "Send anyway" starts the turn. With multiple follow-ups, rejection or a
// failed confirmed turn must park the remainder rather than arm idle delivery.
// Owner: interactive TUI
pub(super) const SEND_CONFIRM_HANDOFF_ID: &str = "send_confirm_handoff";

pub(crate) fn is_send_confirm_scenario(name: &str) -> bool {
    name == SEND_CONFIRM_HANDOFF_ID
}

const SEED_PROMPT: &str = "hello there";
const SEND_PROMPT: &str = "confirmed prompt";

#[derive(Clone, Copy)]
enum QueuedStop {
    Rejected,
    Failed,
}

fn assert_queued_stop(
    harness: &mut crate::harness::PtyHarness,
    stop: QueuedStop,
) -> anyhow::Result<()> {
    let case = match stop {
        QueuedStop::Rejected => "reject",
        QueuedStop::Failed => "fail",
    };
    let prompt = format!("fixture queued confirmation {case}");
    let parked = format!("parked {case} follow-up");
    harness.set_phase(format!("queue_before_{case}"));
    harness.submit_text(&prompt)?;
    harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
    harness.inject_key(&Key::Char('1'))?;
    harness.wait_for_text(&format!("queue checkpoint: {prompt}"), STREAM)?;
    harness.type_text("fixture stream failure")?;
    harness.inject_key(&Key::AltEnter)?;
    harness.wait_for_text("1 follow-up", SETTLE)?;
    harness.type_text(&parked)?;
    harness.inject_key(&Key::CtrlEnter)?;
    harness.wait_for_text("2 follow-ups", SETTLE)?;
    super::release_fixture(harness, ".rho-fixture-release-send-confirm")?;
    harness.wait_for_text("Send to openai/gpt-5.5?", STREAM)?;

    harness.set_phase(format!("confirmed_queue_{case}"));
    match stop {
        QueuedStop::Rejected => {
            // Esc cancels regardless of which options the modal has drawn yet.
            harness.inject_key(&Key::Esc)?;
            harness.wait_for_text("send cancelled", SETTLE)?;
            harness.assert_screen_contains("fixture stream failure")?;
        }
        QueuedStop::Failed => {
            harness.inject_key(&Key::Char('1'))?;
            harness.wait_for_text("deterministic forced stream termination", STREAM)?;
        }
    }

    // A unique local-command receipt proves idle input ran after the boundary.
    // The broken idle arm instead consumes the parked prompt into another modal.
    harness.inject_key(&Key::Ctrl('c'))?;
    harness.submit_text(&format!("!!printf '%s%s\\n' queue-stopped- {case}"))?;
    harness.wait_for_text(&format!("queue-stopped-{case}"), STREAM)?;
    harness.assert_screen_contains("1 follow-up")?;
    harness.assert_screen_contains(&parked)?;
    anyhow::ensure!(
        !harness.screen().contains_text("Send to openai/gpt-5.5?"),
        "idle continuation consumed a parked follow-up after {case}"
    );

    // Retract the parked item rather than send it, keeping the next case isolated.
    harness.inject_key(&Key::AltUp)?;
    harness.wait_for_text("editing queued follow-up", SETTLE)?;
    harness.inject_key(&Key::Ctrl('c'))?;
    Ok(())
}

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
        harness.submit_text(SEND_PROMPT)?;
        harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
        harness.assert_screen_contains("Send anyway")?;
        harness.assert_screen_contains("Don't send")?;

        harness.set_phase("esc_returns_prompt");
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text("send cancelled", SETTLE)?;
        harness.assert_screen_contains(SEND_PROMPT)?;

        harness.set_phase("send_anyway_starts_turn");
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("Send to openai/gpt-5.5?", SETTLE)?;
        harness.inject_key(&Key::Char('1'))?;
        harness.wait_for_text(&format!("fixture response: {SEND_PROMPT}"), STREAM)?;
        for stop in [QueuedStop::Rejected, QueuedStop::Failed] {
            assert_queued_stop(&mut harness, stop)?;
        }
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
