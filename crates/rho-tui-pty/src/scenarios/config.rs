use super::*;

pub(super) const PERMISSIONS_COMMAND_SCENARIO: Scenario = Scenario::new(
    "permissions_command",
    "Change permission modes by slash command without skipping the classifier gate",
    PtySize {
        rows: 14,
        cols: 100,
    },
    PERMISSIONS_COMMAND_STEPS,
    /*smoke*/ false,
)
.with_env(OPENAI_KEY_ENV);

// Covers: slash mode changes preserve the Auto classifier gate and reject
// invalid input without replacing the active policy. Auto setup then offers
// the screen model, and Esc keeps the classifier model on the screen.
// Owner: interactive TUI
const PERMISSIONS_COMMAND_STEPS: &[Step] = &[
    Step::WaitText {
        text: "Bypass ·",
        timeout: STARTUP,
    },
    Step::SubmitText("/permissions auto"),
    Step::WaitText {
        text: "select model for permission-classifier",
        timeout: SETTLE,
    },
    Step::AssertText("Bypass ·"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "permission mode stays bypass",
        timeout: SETTLE,
    },
    Step::SubmitText("/permissions auto"),
    Step::WaitText {
        text: "select model for permission-classifier",
        timeout: SETTLE,
    },
    Step::TypeIntoField("gpt-5.5"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Select permission screen model",
        timeout: SETTLE,
    },
    Step::AssertText("Same as classifier"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Select permission screen model",
        timeout: SETTLE,
    },
    Step::AssertText("Auto ·"),
    Step::SubmitText("/permissions plan"),
    Step::WaitText {
        text: "Plan ·",
        timeout: SETTLE,
    },
    Step::SubmitText("/permissions bypass extra"),
    Step::WaitText {
        text: "could not change permission mode",
        timeout: SETTLE,
    },
    Step::AssertText("Plan ·"),
    Step::SubmitText("/permissions supervised"),
    Step::WaitText {
        text: "Supervised ·",
        timeout: SETTLE,
    },
    Step::SubmitText("/permissions allow_edits"),
    Step::WaitText {
        text: "Allow edits ·",
        timeout: SETTLE,
    },
    Step::SubmitText("/permissions bypass"),
    Step::WaitText {
        text: "Bypass ·",
        timeout: SETTLE,
    },
    Step::SubmitText("/permissions"),
    Step::WaitText {
        text: "permission mode: bypass. usage:",
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
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[behavior]
permission_mode = "auto"
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
// Covers: the same resume when the daemon holds the session.
// Owner: interactive TUI as a daemon client
#[cfg(unix)]
pub(super) const DAEMON_AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID: &str =
    "daemon_auto_permission_mode_recovered_handoff";

pub(super) const AUTO_RECOVERED_HANDOFF_SCENARIO: Scenario = Scenario::new(
    AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID,
    "Resume a session under Auto without a classifier and force the model pick",
    AUTO_RECOVERED_HANDOFF_SIZE,
    &[],
    /*smoke*/ false,
);
#[cfg(unix)]
pub(super) const DAEMON_AUTO_RECOVERED_HANDOFF_SCENARIO: Scenario = Scenario::new(
    DAEMON_AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID,
    "Resume a daemon session under Auto without a classifier and force the model pick",
    AUTO_RECOVERED_HANDOFF_SIZE,
    &[],
    /*smoke*/ false,
);
const AUTO_RECOVERED_HANDOFF_SIZE: PtySize = PtySize {
    rows: 16,
    cols: 100,
};

/// Two-process scenario: seed a real session (with agent identity), rewrite its
/// assistant message to carry non-replayable provider context, switch config to
/// Auto without a classifier, resume, continue the handoff, then confirm the
/// classifier gate still opens.
// Owner: interactive TUI
pub(super) fn auto_recovered_handoff_scenario(name: &str) -> Option<SessionHost> {
    match name {
        AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID => Some(SessionHost::InProcess),
        #[cfg(unix)]
        DAEMON_AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID => Some(SessionHost::Daemon),
        _ => None,
    }
}

/// Where the recovered-handoff scenario's sessions run.
#[derive(Clone, Copy)]
pub(super) enum SessionHost {
    InProcess,
    #[cfg(unix)]
    Daemon,
}

impl SessionHost {
    fn id(self) -> &'static str {
        match self {
            Self::InProcess => AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID,
            #[cfg(unix)]
            Self::Daemon => DAEMON_AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID,
        }
    }

    fn launch(self, plan: crate::env::RhoLaunchPlan) -> crate::env::RhoLaunchPlan {
        match self {
            Self::InProcess => plan,
            #[cfg(unix)]
            Self::Daemon => plan.with_env("RHO_TUI_DAEMON", "1"),
        }
    }
}

pub(super) fn run_auto_recovered_handoff(
    runner: &crate::scenario::ScenarioRunner,
    host: SessionHost,
) -> anyhow::Result<crate::scenario::ScenarioOutcome> {
    use std::fs;

    use crate::{
        artifacts::ArtifactWriter,
        env::{IsolatedHome, RhoLaunchPlan},
        harness::PtyHarness,
        pty::PtySize,
        scenario::ScenarioOutcome,
    };

    const SIZE: PtySize = AUTO_RECOVERED_HANDOFF_SIZE;
    const SEED_PROMPT: &str = "recovered auto handoff seed";

    let home = IsolatedHome::new()?;
    // Phase 1: create a session under Bypass so the transcript carries a real
    // default-agent fingerprint (seeded JSONL cannot invent one safely).
    fs::write(
        &home.config_path,
        r#"web_search.mode = "off"

[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"

[behavior]
permission_mode = "bypass"
credential_store = "file"
"#,
    )?;
    let seed_plan = host.launch(
        RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
            .with_env("OPENAI_API_KEY", "sk-test-matrix"),
    );
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
            id: host.id().into(),
            passed: false,
            message: format!("seed phase failed: {error:#}"),
            timing: seed.timing().clone(),
            artifact_dir: runner.artifact_root.clone(),
        });
    }

    let (session_id, session_path) = find_latest_session(&home)?;
    // A daemon that hosted the seed still holds its session and config.
    home.stop_daemon();
    inject_non_replayable_provider_context(&session_path, StoredProviderRewrite::ToAnthropic)?;
    setup_auto_without_classifier(&home)?;

    let resume_plan = host.launch(
        RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
            .with_env("OPENAI_API_KEY", "sk-test-matrix")
            .with_arg("--resume")
            .with_arg(&session_id),
    );
    let mut harness = PtyHarness::spawn_named(&resume_plan, host.id())?;
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
            id: host.id().into(),
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
                id: host.id().into(),
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
                // Rewind journals are JSONL too and are often written last.
                if path.file_name().and_then(|name| name.to_str()) != Some("workspace-checkpoints")
                {
                    stack.push(path);
                }
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
            // Full snapshots carry `history`; later commits after a baseline
            // (such as the workspace-rewind root) carry `appended_history`.
            for key in ["history", "appended_history"] {
                if let Some(history) = map.get_mut(key).and_then(|h| h.as_array_mut()) {
                    for item in history.iter_mut() {
                        if rewrite_message_value(item) {
                            count += 1;
                        }
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

// Covers: an in-process TUI, which has no daemon to save settings, answers
// `/config` and `/advisor` by saying how to get one, rather than editing
// local state the next session would not see.
// Owner: interactive TUI settings commands in process
pub(super) const SETTINGS_NEED_THE_DAEMON: Scenario = Scenario::new(
    "settings_need_the_daemon",
    "Ask for /config and /advisor in process and get told they need the daemon",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "Bypass ·",
            timeout: STARTUP,
        },
        Step::SubmitText("/config"),
        Step::WaitText {
            text: "/config needs the rho daemon: start rho with RHO_TUI_DAEMON=1",
            timeout: SETTLE,
        },
        Step::SubmitText("/advisor on"),
        Step::WaitText {
            text: "/advisor needs the rho daemon: start rho with RHO_TUI_DAEMON=1",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_env(IN_PROCESS_ENV);

/// Keeps the TUI in process even when a run forces every launch into the
/// daemon: a later `RHO_TUI_DAEMON` pair wins. The in-process TUI still
/// ships (Windows runs no daemon), so its answers keep their coverage.
const IN_PROCESS_ENV: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "fixture-openai-key"),
    ("RHO_TUI_DAEMON", "0"),
];
