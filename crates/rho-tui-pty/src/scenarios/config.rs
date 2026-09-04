use super::*;

// Covers: enabling Auto without a classifier model asks for one, Esc keeps the
// prior mode, and selecting a classifier completes Auto.
// Owner: interactive TUI
pub(super) const AUTO_PERMISSION_MODE_CONFIG_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "Bypass",
        timeout: STARTUP,
    },
    Step::Phase("auto_without_classifier_opens_model_picker"),
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Config · saves automatically",
        timeout: SETTLE,
    },
    Step::TypeText("agent"),
    Step::WaitText {
        text: "Agent behavior",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Permission mode"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "No permission checks",
        timeout: SETTLE,
    },
    // Bypass → Auto. Short terminals hide the status toast under the picker
    // chrome, so the classifier picker title is the durable wait target.
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "select model for permission-classifier",
        timeout: SETTLE,
    },
    Step::AssertText("openai/gpt-5.5"),
    // Mode must not commit until a classifier is chosen.
    Step::AssertText("Bypass ·"),
    Step::Phase("escape_keeps_bypass"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "permission mode stays bypass: no classifier model selected",
        timeout: SETTLE,
    },
    Step::AssertText("Permission mode"),
    // Auto stays highlighted after cancel, so the detail line describes Auto
    // while Bypass remains the selected/runtime mode.
    Step::AssertText("selected"),
    Step::AssertText("Classifier reviews new files, processes, and outside-workspace reads"),
    Step::AssertText("Bypass ·"),
    Step::Phase("select_classifier_applies_auto"),
    // Cancel returns to the permission-mode list with Auto still highlighted.
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "select model for permission-classifier",
        timeout: SETTLE,
    },
    Step::TypeText("gpt-5.5"),
    Step::WaitText {
        text: "openai/gpt-5.5",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Permission mode"),
    Step::AssertText("Auto"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Appearance",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "permissions: auto",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Auto ·",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

// Covers: launching with Auto and no classifier opens a blocking model picker;
// Esc falls back to Supervised so gated tools still ask a human.
// Owner: interactive TUI
pub(super) fn setup_auto_without_classifier(home: &crate::env::IsolatedHome) -> anyhow::Result<()> {
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"
permission_mode = "auto"

[behavior]
credential_store = "file"
"#,
    )?;
    Ok(())
}

pub(super) const AUTO_PERMISSION_MODE_STARTUP_STEPS: &[Step] = &[
    Step::Phase("startup_opens_classifier_picker"),
    Step::WaitText {
        text: "select model for permission-classifier",
        timeout: STARTUP,
    },
    Step::AssertText("openai/gpt-5.5"),
    // Auto is already the configured mode; the picker blocks until a model is
    // chosen or the user backs out.
    Step::AssertText("Auto ·"),
    Step::Phase("escape_falls_back_to_supervised"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "permission mode set to supervised: no classifier model selected",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Supervised ·",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID: &str =
    "auto_permission_mode_recovered_handoff";

/// Two-process scenario: seed a real session (with agent identity), rewrite its
/// assistant message to carry non-replayable provider context, switch config to
/// Auto without a classifier, resume, continue the handoff, then confirm the
/// classifier gate still opens.
// Owner: interactive TUI
pub(super) fn is_auto_recovered_handoff_scenario(name: &str) -> bool {
    name == AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID
}

pub(super) fn run_auto_recovered_handoff(
    runner: &crate::scenario::ScenarioRunner,
) -> anyhow::Result<crate::scenario::ScenarioOutcome> {
    use std::fs;

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
    const SEED_PROMPT: &str = "recovered auto handoff seed";

    let home = IsolatedHome::new()?;
    // Phase 1: create a session under Bypass so the transcript carries a real
    // default-agent fingerprint (seeded JSONL cannot invent one safely).
    fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search_provider = "disabled"
permission_mode = "bypass"

[behavior]
credential_store = "file"
"#,
    )?;
    let seed_plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix");
    let mut seed = PtyHarness::spawn_named(&seed_plan, "auto_recovered_seed")?;
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
            id: AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID.into(),
            passed: false,
            message: format!("seed phase failed: {error:#}"),
            timing: seed.timing().clone(),
            artifact_dir: runner.artifact_root.clone(),
        });
    }

    let (session_id, session_path) = find_latest_session(&home)?;
    inject_non_replayable_provider_context(&session_path, StoredProviderRewrite::ToAnthropic)?;
    setup_auto_without_classifier(&home)?;

    let resume_plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_arg("--resume")
        .with_arg(&session_id);
    let mut harness =
        PtyHarness::spawn_named(&resume_plan, AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> anyhow::Result<()> {
        harness.set_phase("loaded_session_handoff_opens");
        harness.wait_for_text("How should Rho continue", STARTUP)?;
        harness.assert_screen_contains("Auto ·")?;
        harness.set_phase("continue_handoff_then_classifier_gate");
        // Prefer Enter on the highlighted Continue option; number shortcuts vary
        // when use-source/compact rows are present.
        harness.inject_key(&Key::Enter)?;
        harness.wait_for_text("select model for permission-classifier", SETTLE)?;
        harness.assert_screen_contains("Auto ·")?;
        harness.set_phase("escape_falls_back_to_supervised");
        harness.inject_key(&Key::Esc)?;
        harness.wait_for_text(
            "permission mode set to supervised: no classifier model selected",
            SETTLE,
        )?;
        harness.wait_for_text("Supervised ·", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        if code != 0 {
            anyhow::bail!("resume session exited with code {code}");
        }
        Ok(())
    })();

    Ok(match result {
        Ok(()) => ScenarioOutcome {
            id: AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID.into(),
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
                id: AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID.into(),
                passed: false,
                message: format!("{error:#}"),
                timing: harness.timing().clone(),
                artifact_dir: runner.artifact_root.clone(),
            }
        }
    })
}

pub(super) fn find_latest_session(
    home: &crate::env::IsolatedHome,
) -> anyhow::Result<(String, std::path::PathBuf)> {
    use std::fs;

    let root = home.home.join(".rho/sessions");
    let mut latest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) != Some("session.jsonl")
                && path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
            {
                continue;
            }
            let modified = entry.metadata()?.modified()?;
            if latest
                .as_ref()
                .map(|(time, _)| modified > *time)
                .unwrap_or(true)
            {
                latest = Some((modified, path));
            }
        }
    }
    let path = latest
        .map(|(_, path)| path)
        .ok_or_else(|| anyhow::anyhow!("no session created during seed phase"))?;
    let header = fs::read_to_string(&path)?
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("session file empty"))?
        .to_string();
    let value: serde_json::Value = serde_json::from_str(&header)?;
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("session header missing id"))?
        .to_string();
    Ok((id, path))
}

/// Whether session JSON should also rewrite stored provider identity to
/// anthropic. Send-confirm keeps the stored openai provider so resume stays on
/// the original runtime while assistant history still carries native blocks.
#[derive(Clone, Copy)]
pub(super) enum StoredProviderRewrite {
    Keep,
    ToAnthropic,
}

pub(super) fn inject_non_replayable_provider_context(
    path: &std::path::Path,
    stored_provider: StoredProviderRewrite,
) -> anyhow::Result<()> {
    use std::fs;

    let raw = fs::read_to_string(path)?;
    let mut out = Vec::new();
    let mut rewritten = 0usize;
    for line in raw.lines() {
        let mut value: serde_json::Value = serde_json::from_str(line)?;
        rewritten += rewrite_history_tree(&mut value, stored_provider);
        out.push(serde_json::to_string(&value)?);
    }
    if rewritten == 0 {
        anyhow::bail!(
            "could not rewrite any assistant messages in {} for native-context omissions",
            path.display()
        );
    }
    fs::write(path, out.join("\n") + "\n")?;
    Ok(())
}

fn rewrite_history_tree(
    value: &mut serde_json::Value,
    stored_provider: StoredProviderRewrite,
) -> usize {
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
            if matches!(stored_provider, StoredProviderRewrite::ToAnthropic) {
                if let Some(provider) = map.get_mut("provider") {
                    if provider.get("provider").and_then(|v| v.as_str()) == Some("openai")
                        || provider.get("api").and_then(|v| v.as_str()) == Some("tui-test-fixture")
                    {
                        *provider = serde_json::json!({
                            "provider": "anthropic",
                            "api": "messages",
                            "model": "claude-fable-5"
                        });
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
                count += rewrite_history_tree(child, stored_provider);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                count += rewrite_history_tree(item, stored_provider);
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

pub(super) const OPEN_CONFIG_PICKER_STEPS: &[Step] = &[
    Step::Phase("open_config"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Appearance",
        timeout: SETTLE,
    },
    Step::AssertText("Models"),
    Step::AssertText("Agent behavior"),
    Step::AssertText("Context & limits"),
    Step::AssertText("Tools"),
    Step::AssertText("Providers"),
    Step::Phase("open_models"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Conversation model",
        timeout: SETTLE,
    },
    Step::AssertText("Reasoning"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Appearance",
        timeout: SETTLE,
    },
    Step::Phase("open_appearance"),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Theme",
        timeout: SETTLE,
    },
    Step::AssertText("Zen mode"),
    Step::AssertText("Show reasoning output"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Agent behavior",
        timeout: SETTLE,
    },
    Step::Phase("open_refresh_models"),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Log in to provider",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "All configured providers",
        timeout: SETTLE,
    },
    // Running the refresh would ask real provider endpoints what they host, so
    // the scenario stops at the choice and leaves the network out of it.
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Refresh model lists",
        timeout: SETTLE,
    },
    Step::AssertText("Refresh models.dev catalog"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Appearance",
        timeout: SETTLE,
    },
    Step::Phase("select_edit_tool"),
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Inline shell",
        timeout: SETTLE,
    },
    Step::AssertText("Edit tool"),
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "str_replace",
        timeout: SETTLE,
    },
    Step::TypeText("apply_patch"),
    Step::WaitTextGone {
        text: "str_replace",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Config / Tools",
        timeout: SETTLE,
    },
    Step::AssertText("apply_patch"),
    Step::Key(Key::Esc),
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
