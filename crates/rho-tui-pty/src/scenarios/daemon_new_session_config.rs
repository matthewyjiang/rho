//! `/new` opens its session on `config.toml` as it is then, not as the TUI
//! read it at launch, under the launch's flags.

use anyhow::{ensure, Result};

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "daemon_new_session_config",
    "Open each new daemon session on the config.toml of the moment, under the launch's flags",
    DEFAULT_SIZE,
    &[],
    /*smoke*/ false,
);

/// A daemon-hosted fixture session on `provider`/`model`, in bypass mode.
fn write_config(home: &IsolatedHome, provider: &str, model: &str) -> Result<()> {
    std::fs::write(
        &home.config_path,
        format!(
            r#"web_search.mode = "off"

[model]
provider = "{provider}"
model = "{model}"
auth = "api-key"

[behavior]
permission_mode = "bypass"
credential_store = "file"
"#
        ),
    )?;
    Ok(())
}

// Covers: `/new` after a config.toml edit shows and opens its session on
// the edited model, while the launch's `--permission-mode` still holds over
// the config's, and a pick in a session holds in the next while config.toml
// leaves it alone; a config.toml that no longer loads fails the next
// session's first prompt with the daemon's error naming the file.
// Owner: interactive TUI as a daemon client
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    write_config(&home, "openai", "gpt-5.5")?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, DEFAULT_SIZE)
        .with_arg("--permission-mode")
        .with_arg("supervised")
        .with_env("OPENAI_API_KEY", "fixture-openai-key")
        .with_env("XAI_API_KEY", "fixture-xai-key");
    let mut harness = PtyHarness::spawn_named(&plan, SCENARIO.id)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        harness.set_phase("launch_session");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.assert_screen_contains("Supervised ·")?;
        harness.submit_text("hello launch")?;
        harness.wait_for_text("fixture response: hello launch", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;

        harness.set_phase("new_session_after_edit");
        write_config(&home, "xai", "grok-4.6")?;
        harness.submit_text("/new")?;
        harness.wait_for_text("new session", SETTLE)?;
        harness.wait_for_text("grok-4.6", SETTLE)?;
        // The turn's end reports the session's own options, so these show
        // the edited model only if the daemon opened the session on it.
        harness.submit_text("hello edited")?;
        harness.wait_for_text("fixture response: hello edited", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        harness.assert_screen_contains("grok-4.6")?;
        harness.assert_screen_contains("Supervised ·")?;
        ensure!(
            !harness.screen().contains_text("gpt-5.5"),
            "the new session ran on the launch's model:\n{}",
            harness.screen().contents()
        );

        // A pick holds in the next session while config.toml leaves the
        // setting alone, over the launch's flag too.
        harness.set_phase("pick_holds_across_new");
        harness.submit_text("/permissions bypass")?;
        harness.wait_for_text("Bypass ·", SETTLE)?;
        harness.submit_text("/new")?;
        // The new session starts on an empty transcript.
        harness.wait_for_text_gone("fixture response: hello edited", SETTLE)?;
        harness.submit_text("hello picked")?;
        harness.wait_for_text("fixture response: hello picked", STREAM)?;
        harness.wait_for_text("Type a message", SETTLE)?;
        harness.assert_screen_contains("Bypass ·")?;
        harness.assert_screen_contains("grok-4.6")?;

        harness.set_phase("new_session_on_broken_config");
        std::fs::write(&home.config_path, "[model\n")?;
        harness.submit_text("/new")?;
        harness.wait_for_text_gone("fixture response: hello picked", SETTLE)?;
        harness.submit_text("hello broken")?;
        harness.wait_for_text("invalid config file", STREAM)?;
        // The composer gets the prompt back; clear it to quit.
        harness.inject_key(&Key::Ctrl('c'))?;
        harness.wait_for_text_gone("hello broken", SETTLE)?;
        let code = harness.quit_with_exit_command()?;
        ensure!(code == 0, "the TUI exited with code {code}");
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
