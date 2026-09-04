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

/// Rewrite assistant history to hold anthropic-native context while the stored
/// provider stays `openai`, so resuming on openai/gpt-5.5 leaves native blocks
/// the runtime cannot replay.
fn inject_non_replayable_provider_context(path: &std::path::Path) -> anyhow::Result<()> {
    use std::fs;

    let raw = fs::read_to_string(path)?;
    let mut out = Vec::new();
    let mut rewritten = 0usize;
    for line in raw.lines() {
        let mut value: serde_json::Value = serde_json::from_str(line)?;
        rewritten += rewrite_history_tree(&mut value);
        out.push(serde_json::to_string(&value)?);
    }
    if rewritten == 0 {
        anyhow::bail!(
            "could not rewrite any assistant messages in {} for send-confirm omissions",
            path.display()
        );
    }
    fs::write(path, out.join("\n") + "\n")?;
    Ok(())
}

fn rewrite_history_tree(value: &mut serde_json::Value) -> usize {
    let mut count = 0;
    match value {
        serde_json::Value::Object(map) => {
            if let Some(history) = map.get_mut("history").and_then(|h| h.as_array_mut()) {
                for item in history.iter_mut() {
                    if rewrite_message_value(item) {
                        count += 1;
                    }
                }
            }
            if let Some(display) = map
                .get_mut("display_messages")
                .and_then(|h| h.as_array_mut())
            {
                for item in display.iter_mut() {
                    if let Some(message) = item.get_mut("message") {
                        if rewrite_message_value(message) {
                            count += 1;
                        }
                    }
                }
            }
            for child in map.values_mut() {
                count += rewrite_history_tree(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                count += rewrite_history_tree(item);
            }
        }
        _ => {}
    }
    count
}

fn rewrite_message_value(value: &mut serde_json::Value) -> bool {
    let is_assistant = value.get("Assistant").is_some() || value.get("EnrichedAssistant").is_some();
    if !is_assistant {
        return false;
    }
    *value = serde_json::json!({
        "EnrichedAssistant": {
            "content": [{"Text": "prior answer"}],
            "provenance": {
                "provider": "anthropic",
                "api": "messages",
                "model": "claude-fable-5"
            },
            "provider_context": [{
                "identity": {
                    "provider": "anthropic",
                    "api": "messages",
                    "model": "claude-fable-5"
                },
                "kind": "anthropic_message",
                "data": {"opaque": true}
            }]
        }
    });
    true
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
    inject_non_replayable_provider_context(&session_path)?;

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
