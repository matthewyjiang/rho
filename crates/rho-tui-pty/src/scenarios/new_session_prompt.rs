//! A newborn store must not replace /new's live prompt with an empty snapshot.

use anyhow::Result;

use super::{
    assert_helpers::{saved_session_with_text, wait_for_turn_completion_after},
    DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
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
    let id = saved_session_with_text(harness, "fixture system prompt probe after new")?;

    harness.submit_text("/new")?;
    harness.wait_for_text_gone("system prompt present: after new", SETTLE)?;
    harness.submit_text(&format!("/resume {id}"))?;
    harness.wait_for_text("system prompt present: after new", SETTLE)?;
    probe(harness, "after resume")
}
