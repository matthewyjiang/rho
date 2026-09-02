//! `/thermos` slash-command scenario.

use anyhow::Result;

use crate::{harness::PtyHarness, pty::PtySize, scenario::Scenario, scenario::Step};

use super::STARTUP;

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

// Covers: /thermos must fail closed when the workspace has no
// thermo-nuclear-review workflow, instead of opening /workflow.
// Owner: interactive TUI
const THERMOS_MISSING_WORKFLOW_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("submit"),
    Step::SubmitText("/thermos"),
    Step::WaitText {
        text: "could not start thermos",
        timeout: STARTUP,
    },
    Step::Custom(assert_workflow_hub_did_not_open),
    Step::ExitCommand,
];

pub(super) const THERMOS_MISSING_WORKFLOW_SCENARIO: Scenario = Scenario::new(
    "thermos_missing_workflow",
    "Refuse /thermos when the review workflow is absent",
    SIZE,
    THERMOS_MISSING_WORKFLOW_STEPS,
    /* smoke */ false,
);

fn assert_workflow_hub_did_not_open(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("WORKFLOWS") {
        anyhow::bail!("/thermos opened the workflow hub:\n{screen}");
    }
    Ok(())
}
