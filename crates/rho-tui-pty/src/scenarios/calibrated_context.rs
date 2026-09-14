//! Provider usage must survive idle settlement and drive the next prompt's compact.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};

use crate::{
    artifacts::ArtifactWriter,
    env::{IsolatedHome, RhoLaunchPlan},
    harness::PtyHarness,
    pty::PtySize,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner},
    timing::TimingSummary,
};

use super::{config::find_latest_session, SETTLE, STARTUP, STREAM};

// Covers: idle UI replaces provider calibration with chars/4, preventing the next
// prompt's automatic compact. Existing compact scenarios exercise only /compact.
// Owner: interactive lifecycle and its saved continuation, not SDK token policy.
pub(super) const SCENARIO: Scenario = Scenario::new(
    "calibrated_context_auto_compact",
    "Retain calibrated idle usage, auto-compact on the next prompt, and repeat after resume",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[],
    /*smoke*/ false,
);

pub(super) fn run(runner: &ScenarioRunner) -> Result<ScenarioOutcome> {
    let home = IsolatedHome::new()?;
    let mut config = fs::read_to_string(&home.config_path)?;
    config.push_str(
        "\n[compaction]\nauto_compact = true\ncompact_threshold_percent = 75\ncompact_target_percent = 50\n",
    );
    fs::write(&home.config_path, config)?;
    fs::write(
        home.home.join(".rho/models.toml"),
        "[models.\"openai/gpt-5.5\"]\nusable_context_window = 131072\n",
    )?;
    let plan = RhoLaunchPlan::matrix(&runner.binary, &home, SCENARIO.size)
        .with_env("OPENAI_API_KEY", "sk-test-matrix")
        .with_arg("--no-system-prompt")
        .with_arg("--no-tools");
    let mut timing = TimingSummary::default();
    let result = (|| -> Result<()> {
        let mut plan = plan;
        for phase in ["fresh", "resumed"] {
            let mut harness = PtyHarness::spawn_named(&plan, format!("{}_{}", SCENARIO.id, phase))?;
            harness.enable_timing(runner.record_timing);
            if let Some(root) = &runner.artifact_root {
                harness.set_artifact_writer(ArtifactWriter::new(root));
            }
            let result = (|| -> Result<()> {
                harness.wait_for_text("gpt-5.5", STARTUP)?;
                // Resume restores the genuine saved snapshot. Usage calibration
                // is transient, so the first fresh usage receipt must seed it again.
                harness.set_phase(format!("{phase}_seed_history"));
                harness.submit_text(&format!("fixture calibrated context history {phase}"))?;
                wait_for_idle_reply(
                    &mut harness,
                    &format!("Calibration history ready: {phase}."),
                )?;
                let (session_id, path) = find_latest_session(&home)?;
                let before = completed_compactions(&path)?;

                harness.set_phase(format!("{phase}_calibrated_idle"));
                harness.submit_text(&format!("fixture calibrated context usage {phase}"))?;
                wait_for_idle_reply(
                    &mut harness,
                    &format!("Calibrated response complete: {phase}."),
                )?;
                // 100,000 / 131,072 = 76.3%. Wait only after the latest reply's
                // durable idle footer, never the temporary streaming usage frame.
                harness.wait_for_text("100.0K (76.3%)", SETTLE)?;

                harness.set_phase(format!("{phase}_automatic_compact"));
                let prompt = format!("continue calibrated {phase}");
                harness.submit_text(&prompt)?;
                wait_for_idle_reply(&mut harness, &format!("fixture response: {prompt}"))?;
                let rows = harness.screen().rows_text();
                ensure!(
                    rows.iter().any(|row| row.contains("compact"))
                        && rows.iter().any(|row| {
                            row.contains('→') && row.contains("tokens") && row.contains('−')
                        }),
                    "successful compact card with a token reduction is missing:\n{}",
                    harness.screen().contents()
                );
                ensure!(
                    harness.quit_with_exit_command()? == 0,
                    "{phase} session did not exit cleanly"
                );
                // Shutdown joins persistence. No file polling, sleeps, or
                // synthetic snapshot edits are needed to prove the durable effect.
                let after = completed_compactions(&path)?;
                ensure!(
                    after > before,
                    "{phase} completed compactions did not increase: before={before}, after={after}"
                );
                plan = plan.clone().with_arg("--resume").with_arg(session_id);
                Ok(())
            })();
            timing.samples.extend(harness.timing().samples.clone());
            if result.is_err() && harness.is_running() {
                let _ = harness.kill();
            }
            result?;
        }
        Ok(())
    })();
    Ok(ScenarioOutcome {
        id: SCENARIO.id.into(),
        passed: result.is_ok(),
        message: result
            .as_ref()
            .map_or_else(|error| format!("{error:#}"), |()| "ok".into()),
        timing,
        artifact_dir: result.err().and(runner.artifact_root.clone()),
    })
}

fn wait_for_idle_reply(harness: &mut PtyHarness, reply: &str) -> Result<()> {
    harness.wait_for_text(reply, STREAM)?;
    let deadline = Instant::now() + STREAM.duration;
    loop {
        // The footer must follow this reply, not a previous turn's footer.
        let screen = harness.screen().contents();
        if screen
            .rsplit_once(reply)
            .is_some_and(|(_, tail)| tail.contains("Worked for"))
        {
            return Ok(());
        }
        ensure!(
            harness.is_running(),
            "child exited before idle reply {reply:?}"
        );
        ensure!(
            Instant::now() < deadline,
            "timeout waiting for idle reply {reply:?}:\n{screen}"
        );
        harness.poll(Duration::from_millis(25));
    }
}

fn completed_compactions(path: &Path) -> Result<u64> {
    let contents = fs::read_to_string(path)?;
    for line in contents.lines().rev() {
        let entry: serde_json::Value = serde_json::from_str(line)?;
        let transition = &entry["transition"];
        let state = transition
            .get("snapshot")
            .or_else(|| transition.get("delta"));
        if let Some(state) = state {
            return state["compaction"]["completed_compactions"]
                .as_u64()
                .context("saved snapshot has no completed compaction count");
        }
    }
    anyhow::bail!("no saved snapshot in {}", path.display())
}
