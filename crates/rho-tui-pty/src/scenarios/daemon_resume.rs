//! `RHO_TUI_DAEMON=1 rho --resume <id>` follows the saved session in the
//! daemon and shows its conversation as the in-process TUI did (#1667).

use anyhow::{Context, Result};

use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    pty::PtySize,
    scenario::{ScenarioOutcome, ScenarioRunner},
};

use super::{config, STARTUP, STREAM};

pub(super) const DAEMON_RESUME_ID: &str = "daemon_resume";

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};
const SEED_PROMPT: &str = "fixture tool";
/// The fixture's answer once its `write` call returns.
const ANSWER: &str = "tool lifecycle complete with one result";
/// The file the fixture's `write` call names, which its card shows.
const WRITTEN_FILE: &str = ".rho-tui-fixture-output.txt";

pub(super) fn is_daemon_resume_scenario(name: &str) -> bool {
    name == DAEMON_RESUME_ID
}

// Covers: a daemon-hosted TUI resuming a saved session shows its prompt, the
// daemon's tool card reading as the in-process TUI drew it, and the answer.
// Owner: interactive TUI as a daemon client
pub(super) fn run_daemon_resume(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    // Dropped after the harnesses, so the daemon the resume started stops
    // once nothing is attached to it.
    let home = IsolatedHome::new()?;
    write_daemon_config(&home)?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix");
    let spawn = |plan: &RhoLaunchPlan| -> Result<PtyHarness> {
        let mut harness = PtyHarness::spawn_named(plan, DAEMON_RESUME_ID)?;
        harness.enable_timing(runner.record_timing);
        if let Some(root) = &runner.artifact_root {
            harness.set_artifact_writer(ArtifactWriter::new(root));
        }
        Ok(harness)
    };

    // Seeded in process, so the card the resume shows is checked against
    // the one the in-process TUI drew.
    let mut harness = spawn(&plan)?;
    let seeded = (|| -> Result<String> {
        harness.set_phase("seed_in_process");
        harness.wait_for_text("gpt-5.5", STARTUP)?;
        harness.submit_text(SEED_PROMPT)?;
        harness.wait_for_text(ANSWER, STREAM)?;
        let card = card_header(&harness.screen().contents())
            .context("the in-process transcript shows no write card")?;
        let code = harness.quit_with_exit_command()?;
        anyhow::ensure!(code == 0, "seed session exited with code {code}");
        Ok(card)
    })();

    let result = seeded.and_then(|card| {
        let (session_id, _) = config::find_latest_session(&home)?;
        harness = spawn(
            &plan
                .clone()
                .with_env("RHO_TUI_DAEMON", "1")
                .with_arg("--resume")
                .with_arg(&session_id),
        )?;
        harness.set_phase("resume_in_daemon");
        harness.wait_for_text(ANSWER, STARTUP)?;
        // The replay arrives in one batch, so the answer's frame holds the
        // rows before it too.
        harness.wait_for_text(&card, STREAM)?;
        harness.wait_for_text(SEED_PROMPT, STREAM)?;
        let code = harness.quit_with_exit_command()?;
        anyhow::ensure!(code == 0, "daemon session exited with code {code}");
        Ok(())
    });

    if result.is_err() && harness.is_running() {
        let _ = harness.kill();
    }
    Ok(ScenarioOutcome {
        id: DAEMON_RESUME_ID.into(),
        passed: result.is_ok(),
        message: result
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default(),
        timing: harness.timing().clone(),
        artifact_dir: runner.artifact_root.clone(),
    })
}

/// Config for a daemon-hosted fixture session whose tools run without
/// asking.
pub(super) fn write_daemon_config(home: &IsolatedHome) -> Result<()> {
    write_daemon_config_in_mode(home, "bypass")
}

/// Config for a daemon-hosted fixture session in `permission_mode`.
pub(super) fn write_daemon_config_in_mode(
    home: &IsolatedHome,
    permission_mode: &str,
) -> Result<()> {
    std::fs::write(
        &home.config_path,
        format!(
            r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"
permission_mode = "{permission_mode}"

[behavior]
credential_store = "file"
"#
        ),
    )?;
    Ok(())
}

/// The write card's header, through the file it names: the first row naming
/// the written file that is not the answer, which quotes the tool's result.
/// Cut there so facts after the path cannot make the two renders differ.
fn card_header(screen: &str) -> Option<String> {
    screen
        .lines()
        .map(str::trim)
        .filter(|row| !row.contains(ANSWER))
        .find_map(|row| {
            let end = row.find(WRITTEN_FILE)? + WRITTEN_FILE.len();
            Some(row[..end].to_owned())
        })
}
