//! A session file from rho before 1.11 (format 1-3) is refused with the
//! release that still opens it, in process and in the daemon, and its
//! presence does not break the resume picker.

use std::fs;

use anyhow::{Context, Result};

use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    pty::PtySize,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
};

use super::{
    daemon_resume::write_daemon_config,
    sessions_hub::{workspace_key, write_seed_session},
    STARTUP, STREAM,
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "pre_tree_session",
    "Refuse a pre-1.11 session file by naming rho 2.x, in process and in the daemon",
    SIZE,
    &[],
    /*smoke*/ false,
);

const SIZE: PtySize = PtySize {
    rows: 30,
    cols: 120,
};
const LEGACY_ID: &str = "22222222-2222-4222-8222-222222222222";
const CURRENT_ID: &str = "55555555-0000-4000-8000-000000000005";
const CURRENT_PROMPT: &str = "current session beside an old one";
const REFUSAL: &str = "can't be opened in rho 3; open it with rho 2.x";

/// A format 2 transcript as rho 1.x wrote it, minus its workspace.
const LEGACY_TRANSCRIPT: &str = r#"{"type":"session","version":2,"id":"22222222-2222-4222-8222-222222222222","timestamp":"200","cwd":"WORKSPACE"}
{"type":"message","timestamp":"201","message":{"User":[{"Text":"prompt from rho 1.x"}]}}
{"type":"message","timestamp":"202","message":{"Assistant":[{"Text":"answer from rho 1.x"}]}}
"#;

fn setup(home: &IsolatedHome) -> Result<()> {
    write_daemon_config(home)?;
    write_seed_session(
        home,
        &home.workspace,
        &home.workspace,
        CURRENT_ID,
        &[CURRENT_PROMPT],
    )?;
    let workspace = home.workspace.to_str().context("workspace path is UTF-8")?;
    let workspace = serde_json::to_string(workspace)?;
    fs::write(
        home.home
            .join(".rho/sessions")
            .join(workspace_key(&home.workspace))
            .join(format!("200_{LEGACY_ID}.jsonl")),
        LEGACY_TRANSCRIPT.replace("\"WORKSPACE\"", &workspace),
    )
    .context("write the pre-1.11 transcript")
}

// Covers: `--resume` of a format 2 session exits naming rho 2.x instead of a
// parse error, and the resume picker still lists the current session, both
// in process and as a daemon client.
// Owner: session persistence, through both TUI hosts
pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    setup(&home)?;
    let in_process = RhoLaunchPlan::matrix(&runner.binary, &home, SIZE)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_env("RHO_TUI_DAEMON", "0");
    let spawn = |plan: &RhoLaunchPlan| -> Result<PtyHarness> {
        let mut harness = PtyHarness::spawn_named(plan, SCENARIO.id)?;
        harness.enable_timing(runner.record_timing);
        if let Some(root) = &runner.artifact_root {
            harness.set_artifact_writer(ArtifactWriter::new(root));
        }
        Ok(harness)
    };

    let mut harness: Option<PtyHarness> = None;
    let result = (|| -> Result<()> {
        for (host, plan) in [
            ("in_process", in_process.clone()),
            ("daemon", in_process.clone().with_env("RHO_TUI_DAEMON", "1")),
        ] {
            let resume = plan.clone().with_arg("--resume").with_arg(&LEGACY_ID[..8]);
            let refused = harness.insert(spawn(&resume)?);
            refused.set_phase(format!("{host}_resume_pre_tree"));
            let code = refused.wait_for_exit(STARTUP)?;
            refused.assert_raw_contains(REFUSAL.as_bytes())?;
            anyhow::ensure!(
                code != 0,
                "{host}: resuming a pre-1.11 session exited cleanly"
            );

            let picker = harness.insert(spawn(&plan.clone().with_arg("--resume"))?);
            picker.set_phase(format!("{host}_resume_picker"));
            picker.wait_for_text("Resume session", STARTUP)?;
            picker.wait_for_text(CURRENT_PROMPT, STREAM)?;
            let code = picker.quit_with_exit_command()?;
            anyhow::ensure!(code == 0, "{host}: picker session exited with code {code}");
        }
        Ok(())
    })();

    if let Some(harness) = harness.as_mut().filter(|_| result.is_err()) {
        if harness.is_running() {
            let _ = harness.kill();
        }
    }
    Ok(ScenarioOutcome {
        id: SCENARIO.id.into(),
        passed: result.is_ok(),
        message: result
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default(),
        timing: harness
            .as_ref()
            .map(|harness| harness.timing().clone())
            .unwrap_or_default(),
        artifact_dir: runner.artifact_root.clone(),
    })
}
