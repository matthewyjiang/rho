//! Covers: a workflow that finishes during a model call must keep its row on
//! the rail until the session is given the run's result, and the row must
//! leave with it.
//! Owner: interactive workflow rail. `workflow_background` releases its runs
//! with the session idle, where the result arrives at once, so a row that
//! leaves as its run ends looks the same there.

use std::fs;

use anyhow::{ensure, Context, Result};

use super::{
    assert_helpers::wait_for_turn_completion_after,
    workflow_background::{assert_live_rail, release_gate, setup_workflow},
    DEFAULT_SIZE, STARTUP, STREAM,
};
use crate::{
    scenario::{Scenario, Step},
    PtyHarness,
};

const FINISHED_ROW: &str = "✓ done · 3/3 tasks";
const RESULT: &str = "(pty-background) finished - success";

pub(super) const SCENARIO: Scenario = Scenario::new(
    "workflow_result_rail",
    "Keep a finished workflow on the rail while its result waits for a busy turn",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(finished_row_waits_for_its_result),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_workflow);

fn finished_row_waits_for_its_result(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("start_background_run");
    harness.submit_text("fixture workflow background")?;
    harness.wait_for_text("workflow fixture dispatched", STREAM)?;
    wait_for_turn_completion_after(harness, "workflow fixture dispatched")?;
    assert_live_rail(harness)?;

    harness.set_phase("finished_row_stays_while_the_turn_runs");
    // One model call that waits for its release, so nothing can take the
    // result until then.
    harness.submit_text("fixture gated reply")?;
    harness.wait_for_text("reply waiting for release", STREAM)?;
    release_gate(harness)?;
    harness.wait_for_text(FINISHED_ROW, STREAM)?;
    ensure!(
        !harness.screen().contains_text(RESULT),
        "the result arrived during a model call:\n{}",
        harness.screen().debug_dump()
    );

    harness.set_phase("row_leaves_with_the_result");
    fs::write(
        harness
            .working_directory()
            .context("workflow workspace")?
            .join(".rho-fixture-release-reply"),
        b"release",
    )?;
    harness.wait_for_text_gone("└ workflow pty-background", STREAM)?;
    // The daemon gives the result to the idle session in a turn of its own.
    harness.wait_for_text(RESULT, STREAM)?;
    wait_for_turn_completion_after(harness, RESULT)
}
