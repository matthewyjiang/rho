//! A newborn store must not replace /new's live prompt with an empty snapshot.

use anyhow::{ensure, Context, Result};

use super::{
    assert_helpers::wait_for_turn_completion_after, DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    scenario::{Scenario, Step},
    PtyHarness,
};

pub(super) const SCENARIO: Scenario = Scenario::new(
    "new_session_system_prompt",
    "Keep the system prompt through /new's first provider request and subsequent resume",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(probe_new_and_resumed_sessions),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

fn probe(harness: &mut PtyHarness, phase: &str) -> Result<()> {
    harness.set_phase(phase);
    harness.submit_text(&format!("fixture system prompt probe {phase}"))?;
    let response = format!("system prompt present: {phase}");
    harness.wait_for_text(&response, STREAM)?;
    wait_for_turn_completion_after(harness, &response)
}

fn probe_new_and_resumed_sessions(harness: &mut PtyHarness) -> Result<()> {
    probe(harness, "startup")?;
    harness.submit_text("/new")?;
    harness.wait_for_text_gone("system prompt present: startup", SETTLE)?;
    probe(harness, "after new")?;
    let id = saved_probe_session(harness)?;

    harness.submit_text("/new")?;
    harness.wait_for_text_gone("system prompt present: after new", SETTLE)?;
    harness.submit_text(&format!("/resume {id}"))?;
    harness.wait_for_text("system prompt present: after new", SETTLE)?;
    probe(harness, "after resume")
}

/// Select the session by its committed probe, not timestamps that can tie.
fn saved_probe_session(harness: &PtyHarness) -> Result<String> {
    let root = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?
        .join("home/.rho/sessions");
    let mut directories = vec![root];
    let mut matching_ids = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
            } else if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                let content = std::fs::read_to_string(entry.path())?;
                if content.contains("fixture system prompt probe after new") {
                    let header: serde_json::Value = serde_json::from_str(
                        content.lines().next().context("session file is empty")?,
                    )?;
                    matching_ids.push(
                        header["id"]
                            .as_str()
                            .context("session header has no id")?
                            .to_owned(),
                    );
                }
            }
        }
    }
    ensure!(
        matching_ids.len() == 1,
        "expected one committed post-/new session, found {}",
        matching_ids.len()
    );
    Ok(matching_ids.remove(0))
}
