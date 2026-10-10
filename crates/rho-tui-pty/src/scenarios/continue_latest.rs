//! `rho -c` resumes the workspace's latest session, or starts fresh without one.

use anyhow::Result;

use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    pty::PtySize,
    scenario::{ScenarioOutcome, ScenarioRunner},
};

use super::{STARTUP, STREAM};

pub(super) const CONTINUE_LATEST_ID: &str = "continue_latest";

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};
const FIRST_PROMPT: &str = "first continued turn";
const SECOND_PROMPT: &str = "second continued turn";

pub(super) fn is_continue_latest_scenario(name: &str) -> bool {
    name == CONTINUE_LATEST_ID
}

// Covers: `-c` on an empty workspace starts a saved session instead of failing,
// and a second `-c` reopens that session (no picker, no duplicate session).
// Owner: interactive startup
pub(super) fn run_continue_latest(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"
permission_mode = "bypass"

[behavior]
credential_store = "file"
"#,
    )?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_arg("-c");
    let spawn = || -> Result<PtyHarness> {
        let mut harness = PtyHarness::spawn_named(&plan, CONTINUE_LATEST_ID)?;
        harness.enable_timing(runner.record_timing);
        if let Some(root) = &runner.artifact_root {
            harness.set_artifact_writer(ArtifactWriter::new(root));
        }
        Ok(harness)
    };

    let mut harness = spawn()?;
    let mut result = (|| -> Result<()> {
        harness.set_phase("empty_workspace_starts_fresh");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        submit_and_quit(&mut harness, FIRST_PROMPT)
    })();
    if result.is_ok() {
        harness = spawn()?;
        result = (|| -> Result<()> {
            harness.set_phase("continue_reopens_latest");
            harness.wait_for_text(&format!("fixture response: {FIRST_PROMPT}"), STARTUP)?;
            submit_and_quit(&mut harness, SECOND_PROMPT)?;
            let sessions = session_transcripts(&home)?;
            anyhow::ensure!(
                sessions == 1,
                "-c must append to the latest session, found {sessions} sessions"
            );
            Ok(())
        })();
    }

    if result.is_err() && harness.is_running() {
        let _ = harness.kill();
    }
    Ok(ScenarioOutcome {
        id: CONTINUE_LATEST_ID.into(),
        passed: result.is_ok(),
        message: result
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default(),
        timing: harness.timing().clone(),
        artifact_dir: runner.artifact_root.clone(),
    })
}

fn submit_and_quit(harness: &mut PtyHarness, prompt: &str) -> Result<()> {
    harness.submit_text(prompt)?;
    harness.wait_for_text(&format!("fixture response: {prompt}"), STREAM)?;
    let code = harness.quit_with_exit_command()?;
    anyhow::ensure!(code == 0, "session exited with code {code}");
    Ok(())
}

/// Counts saved session transcripts under the isolated home.
fn session_transcripts(home: &IsolatedHome) -> Result<usize> {
    let mut count = 0;
    let mut pending = vec![home.home.join(".rho/sessions")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("session.jsonl") {
                count += 1;
            }
        }
    }
    Ok(count)
}
