//! Gated partial write arguments must appear before the real write runs.

use anyhow::{ensure, Context, Result};

use crate::{
    harness::PtyHarness,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP, STREAM};

const TARGET: &str = ".rho-tui-fixture-streamed-write.txt";
const FIRST: &str = "FIRST_WRITE_FRAGMENT";
const SECOND: &str = "SECOND_WRITE_FRAGMENT";

// Covers: write input stays hidden until complete JSON/tool execution, or a
// later argument fragment replaces the already-visible prefix/duplicates cards.
// Owner: interactive TUI. Existing edit_diff does not exercise write content.
pub(super) const WRITE_INPUT_STREAM_SCENARIO: Scenario = Scenario::new(
    "write_input_stream",
    "Show incremental write content before completion and retain one result card",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(exercise_write_stream),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

fn exercise_write_stream(harness: &mut PtyHarness) -> Result<()> {
    let target = harness
        .working_directory()
        .context("missing workspace")?
        .join(TARGET);
    ensure!(
        !target.try_exists()?,
        "write target exists before submission"
    );
    harness.set_phase("first_open_content_fragment");
    harness.submit_text("fixture write input stream")?;
    harness.wait_for_text(FIRST, STREAM)?;
    ensure!(
        !target.try_exists()?,
        "write executed before the first partial-content assertion"
    );
    ensure!(
        !harness.screen().contains_text(SECOND),
        "second content fragment arrived before its release"
    );
    assert_one_card(harness)?;

    harness.set_phase("second_open_content_fragment");
    super::release_fixture(harness, ".rho-fixture-release-write-next")?;
    harness.wait_for_text(SECOND, STREAM)?;
    ensure!(
        harness.screen().contains_text(FIRST),
        "first write fragment disappeared when the second arrived"
    );
    ensure!(
        !target.try_exists()?,
        "write executed while its content string was still open"
    );
    assert_one_card(harness)?;

    harness.set_phase("completed_write");
    super::release_fixture(harness, ".rho-fixture-release-write-complete")?;
    harness.wait_for_text("streamed write completed successfully", STREAM)?;
    // The completion notice can paint before the card body in a PTY read.
    harness.wait_for_text(SECOND, STREAM)?;
    ensure!(
        harness.screen().contains_text(FIRST),
        "completed write lost the first content fragment"
    );
    assert_one_card(harness)?;
    ensure!(
        std::fs::read_to_string(&target)? == "FIRST_WRITE_FRAGMENT\nSECOND_WRITE_FRAGMENT\n",
        "completed write did not persist the complete streamed payload"
    );
    Ok(())
}

fn assert_one_card(harness: &PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let count = screen.matches("write(").count();
    ensure!(
        count == 1,
        "expected one write card, found {count}:\n{screen}"
    );
    Ok(())
}
