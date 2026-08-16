//! Resume must keep earlier transcript rows available for scrollback.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    pty::PtySize,
    scenario::{ScenarioOutcome, ScenarioRunner},
};

use super::{config, STARTUP, STREAM};

pub(super) const RESUME_SCROLLBACK_ID: &str = "resume_scrollback";

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};
const SEED_PROMPT: &str = "fixture bulk one";
const EARLY_LINE: &str = "fixture bulk one line 001";
const LATE_LINE: &str = "fixture bulk one line 180";

// Covers: resume must keep earlier transcript rows reachable by page-up.
// Owner: interactive TUI
pub(super) fn is_resume_scrollback_scenario(name: &str) -> bool {
    name == RESUME_SCROLLBACK_ID
}

pub(super) fn run_resume_scrollback(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    std::fs::write(
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
    let mut seed = PtyHarness::spawn_named(&seed_plan, "resume_scrollback_seed")?;
    seed.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        seed.set_artifact_writer(ArtifactWriter::new(root));
    }
    let seed_result = (|| -> Result<()> {
        seed.wait_for_text("gpt-5.5", STARTUP)?;
        seed.submit_text(SEED_PROMPT)?;
        seed.wait_for_text(LATE_LINE, STREAM)?;
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
            id: RESUME_SCROLLBACK_ID.into(),
            passed: false,
            message: format!("seed phase failed: {error:#}"),
            timing: seed.timing().clone(),
            artifact_dir: runner.artifact_root.clone(),
        });
    }

    let (session_id, _) = config::find_latest_session(&home)?;
    let resume_plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_arg("--resume")
        .with_arg(&session_id);
    let mut harness = PtyHarness::spawn_named(&resume_plan, RESUME_SCROLLBACK_ID)?;
    harness.enable_timing(runner.record_timing);
    if let Some(root) = &runner.artifact_root {
        harness.set_artifact_writer(ArtifactWriter::new(root));
    }
    let result = (|| -> Result<()> {
        harness.set_phase("resumed_tail");
        harness.wait_for_text(LATE_LINE, STARTUP)?;
        harness.set_phase("page_to_early_line");
        page_up_until_text(&mut harness, EARLY_LINE, STREAM)?;
        if harness.screen().contains_text("Cycle reasoning") {
            anyhow::bail!("session header tips must stay out of a tall resumed transcript");
        }
        let code = harness.quit_with_exit_command()?;
        if code != 0 {
            anyhow::bail!("resume session exited with code {code}");
        }
        Ok(())
    })();
    Ok(match result {
        Ok(()) => ScenarioOutcome {
            id: RESUME_SCROLLBACK_ID.into(),
            passed: true,
            message: String::new(),
            timing: harness.timing().clone(),
            artifact_dir: runner.artifact_root.clone(),
        },
        Err(error) => {
            if harness.is_running() {
                let _ = harness.kill();
            }
            ScenarioOutcome {
                id: RESUME_SCROLLBACK_ID.into(),
                passed: false,
                message: format!("{error:#}"),
                timing: harness.timing().clone(),
                artifact_dir: runner.artifact_root.clone(),
            }
        }
    })
}

fn page_up_until_text(harness: &mut PtyHarness, needle: &str, timeout: WaitTimeout) -> Result<()> {
    let deadline = Instant::now() + timeout.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        if harness.screen().contains_text(needle) {
            return Ok(());
        }
        if !harness.is_running() {
            anyhow::bail!("child exited before {needle:?} appeared while paging resume history");
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timeout paging resume history to {needle:?}");
        }
        harness.inject_key(&Key::PageUp)?;
    }
}
