//! Covers a process failure pending during tool work being stranded behind completion.
//! Owner: interactive UX. The fixture waits on the process manager's exit signal.
use super::{STARTUP, STREAM};
use crate::{
    pty::PtySize,
    scenario::{Scenario, Step},
};

fn assert_no_early_completion(harness: &mut crate::PtyHarness) -> anyhow::Result<()> {
    let screen = harness.screen().contents();
    anyhow::ensure!(
        !screen.contains("BUG: parent completed"),
        "parent completed before receiving the failure:\n{screen}"
    );
    Ok(())
}

pub(super) const SCENARIO: Scenario = Scenario::new(
    "boundary_notifications",
    "Incorporate a pending background failure before completing the parent turn",
    PtySize {
        rows: 40,
        cols: 110,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture boundary notification"),
        Step::WaitText {
            text: "background failure incorporated before completion",
            timeout: STREAM,
        },
        Step::Custom(assert_no_early_completion),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);
