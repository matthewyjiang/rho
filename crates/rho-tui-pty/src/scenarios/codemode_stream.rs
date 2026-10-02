//! Literal model-generated source must be visible while the script JSON is open.

use anyhow::{ensure, Context, Result};

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{STARTUP, STREAM};

const TARGET: &str = ".rho-tui-fixture-codemode-executed.txt";
const FIRST: &str = "print(\"FIRST_SCRIPT_FRAGMENT\")";
const SECOND: &str = "print(\"SECOND_SCRIPT_FRAGMENT\")";

// Covers: head-clipped cards hide later generated source until execution.
// Owner: interactive TUI. Replaces the completed-call-only codemode_card.
pub(super) const CODEMODE_CARD_SCENARIO: Scenario = Scenario::new(
    "codemode_card",
    "Show literal script generation past the collapsed budget before execution",
    PtySize {
        rows: 40,
        cols: 100,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(exercise_collapsed_stream),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

// Covers: small argument deltas stop updating a script above 4096 bytes.
// Owner: interactive TUI. Expanded mode isolates parsing from collapsed clipping.
pub(super) const CODEMODE_LARGE_SCRIPT_SCENARIO: Scenario = Scenario::new(
    "codemode_large_script_stream",
    "Keep small source deltas live after the argument buffer exceeds 4096 bytes",
    PtySize {
        rows: 40,
        cols: 100,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(exercise_large_stream),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

fn exercise_collapsed_stream(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("first_open_script_fragment");
    harness.submit_text("fixture codemode")?;
    harness.wait_for_text(FIRST, STREAM)?;
    assert_generating_source(harness, FIRST)?;
    assert_one_card(harness)?;
    ensure!(
        !harness.screen().contains_text(SECOND),
        "unreleased script tail appeared"
    );

    harness.set_phase("generated_tail_past_collapsed_budget");
    super::release_fixture(harness, ".rho-fixture-release-codemode-next")?;
    harness.wait_for_text(SECOND, STREAM)?;
    assert_generating_source(harness, SECOND)?;
    assert_one_card(harness)?;

    // The rolling live view must not discard the source prefix. Expansion
    // retrieves it while JSON is still open, not just after execution.
    harness.set_phase("expand_generated_source_history");
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text(FIRST, STREAM)?;
    ensure!(
        harness.screen().contains_text(SECOND),
        "expansion lost the generated tail"
    );
    assert_generating_source(harness, FIRST)?;
    finish(harness)?;
    harness.wait_for_text(FIRST, STREAM)?;
    ensure!(
        harness.screen().contains_text(SECOND),
        "completed card lost source history"
    );
    assert_one_card(harness)
}

fn exercise_large_stream(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("large_open_script_fragment");
    harness.submit_text("fixture codemode large script")?;
    harness.wait_for_text("● codemode", STREAM)?;
    harness.inject_key(&Key::Ctrl('o'))?;
    harness.wait_for_text(FIRST, STREAM)?;
    assert_generating_source(harness, FIRST)?;

    harness.set_phase("small_delta_after_large_argument");
    super::release_fixture(harness, ".rho-fixture-release-codemode-next")?;
    harness.wait_for_text(SECOND, STREAM)?;
    ensure!(
        harness.screen().contains_text(FIRST),
        "small delta lost the previous source row"
    );
    assert_generating_source(harness, SECOND)?;
    finish(harness)
}

fn assert_generating_source(harness: &mut PtyHarness, source: &str) -> Result<()> {
    let target = harness
        .working_directory()
        .context("missing workspace")?
        .join(TARGET);
    ensure!(
        !target.try_exists()?,
        "codemode executed before the script JSON closed"
    );
    let screen = harness.screen().contents();
    ensure!(
        !screen.contains("codemode fixture complete"),
        "provider completed before its release"
    );
    ensure!(
        !screen.contains("{\"script\"") && !screen.contains("\\n") && !screen.contains("\\\""),
        "card printed escaped argument JSON instead of source:\n{screen}"
    );
    let (row, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(source))
        .context("source row missing")?;
    let col = line[..line.find(source).expect("matched source")]
        .chars()
        .count();
    let code = harness
        .screen()
        .cell(row as u16, col as u16)
        .context("missing code cell")?;
    let string = harness
        .screen()
        .cell(row as u16, (col + "print(\"".len()) as u16)
        .context("missing string cell")?;
    ensure!(
        code.fg != string.fg,
        "generated source lost syntax highlighting"
    );
    Ok(())
}

fn finish(harness: &mut PtyHarness) -> Result<()> {
    harness.set_phase("close_script_json_and_execute");
    super::release_fixture(harness, ".rho-fixture-release-codemode-complete")?;
    harness.wait_for_text("codemode fixture complete", STREAM)?;
    harness.wait_for_text("print(\"codemode fixture batch\", len(hits))", STREAM)?;
    let target = harness
        .working_directory()
        .context("missing workspace")?
        .join(TARGET);
    ensure!(
        std::fs::read_to_string(target)? == "executed\n",
        "script did not execute after completion"
    );
    assert_long_line_highlighted(harness)
}

fn assert_one_card(harness: &PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let count = screen
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("● codemode") || line.starts_with("✓ codemode(")
        })
        .count();
    ensure!(
        count == 1,
        "expected one persistent codemode card, found {count}:\n{screen}"
    );
    if screen.contains("codemode fixture complete") {
        ensure!(
            screen.contains("codemode(3 calls)"),
            "completed card lost its nested-call count"
        );
    }
    Ok(())
}
