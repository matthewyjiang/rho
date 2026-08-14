//! Tool-card hover lift and click-to-expand pointer behavior.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::{STARTUP, STREAM};
use crate::{
    harness::WaitTimeout, keys::MouseButton, scenario::Step, screen::CellColor, PtyHarness,
};

/// The rendered look of every text cell in `row`: bold flag plus ink color.
/// The hover lift changes one of the two without changing the text itself.
fn row_look(harness: &PtyHarness, row: u16) -> Vec<(bool, CellColor)> {
    let screen = harness.screen();
    (0..screen.cols())
        .filter_map(|col| {
            let cell = screen.cell(row, col)?;
            (!cell.contents.trim().is_empty()).then_some((cell.bold, cell.fg))
        })
        .collect()
}

/// Poll until `want` holds on `row`, failing with `what` and a dump.
fn wait_for_row_look(
    harness: &mut PtyHarness,
    row: u16,
    want: &dyn Fn(&(bool, CellColor)) -> bool,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        let look = row_look(harness, row);
        if !look.is_empty() && look.iter().all(want) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "{what}; row {row} look: {look:?}\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn prompt_row(harness: &mut PtyHarness) -> Result<u16> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        if let Some(row) = harness
            .screen()
            .rows_text()
            .iter()
            .position(|line| line.contains("more lines, ctrl+o to expand"))
        {
            return Ok(row as u16);
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "collapsed tool card prompt not found:\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Covers: hovering a collapsed tool card lifts its text ink on pointer entry
// and reverts on pointer exit; a completed click still expands the card.
// Owner: interactive UX (PTY).
fn assert_hover_lift_and_click_expand(harness: &mut PtyHarness) -> Result<()> {
    let row = prompt_row(harness)?;
    let baseline = row_look(harness, row);
    assert!(
        !baseline.is_empty(),
        "tool card prompt row has no text cells:\n{}",
        harness.screen().debug_dump()
    );

    // SGR mouse coordinates are 1-based.
    let (column, sgr_row) = (6u16, row + 1);
    harness.mouse_move(column, sgr_row)?;
    wait_for_row_look(
        harness,
        row,
        &|look| {
            *look
                != baseline
                    .first()
                    .copied()
                    .unwrap_or((false, CellColor::Default))
        },
        "hover did not lift the tool card prompt row",
    )?;

    harness.mouse_move(column, 1)?;
    wait_for_row_look(
        harness,
        row,
        &|look| {
            *look
                == baseline
                    .first()
                    .copied()
                    .unwrap_or((false, CellColor::Default))
        },
        "hover lift did not revert after the pointer left the card",
    )?;

    harness.mouse(MouseButton::Left, column, sgr_row, true)?;
    harness.poll(Duration::from_millis(100));
    harness.mouse(MouseButton::Left, column, sgr_row, false)?;
    harness.wait_for_text(
        "ctrl+o to collapse",
        WaitTimeout::secs(2, "click expanded the tool card"),
    )?;
    Ok(())
}

pub(super) const TOOL_CARD_HOVER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("hover_lift_and_click_expand"),
    Step::SubmitText("fixture hover tool"),
    Step::WaitText {
        text: "more lines, ctrl+o to expand",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "hover tool lifecycle complete",
        timeout: STREAM,
    },
    Step::Custom(assert_hover_lift_and_click_expand),
    Step::ExitCommand,
];
