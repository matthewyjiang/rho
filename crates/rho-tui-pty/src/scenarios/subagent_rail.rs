//! Subagent activity-rail pointer behavior.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::{DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    harness::WaitTimeout,
    keys::MouseButton,
    scenario::{Scenario, Step},
    PtyHarness,
};

fn active_subagent_row(harness: &mut PtyHarness) -> Result<u16> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        if let Some(row) = harness
            .screen()
            .rows_text()
            .iter()
            .position(|line| line.contains("└ worker") || line.contains("├ worker"))
        {
            return Ok(row as u16 + 1);
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "active subagent row did not appear:\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn assert_subagent_rail_mouse_flow(harness: &mut PtyHarness) -> Result<()> {
    let column = 3;
    let row = active_subagent_row(harness)?;
    harness.mouse_move(column, row)?;
    harness.wait_for_text(
        "copy attach",
        WaitTimeout::secs(2, "subagent hover action hint"),
    )?;

    let refresh_deadline = Instant::now() + Duration::from_millis(1_200);
    while Instant::now() < refresh_deadline {
        harness.poll(Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(10));
    }
    let hovered_row = harness
        .screen()
        .rows_text()
        .get(row.saturating_sub(1) as usize)
        .cloned()
        .unwrap_or_default();
    if !hovered_row.contains("copy attach") {
        anyhow::bail!("hover disappeared after snapshot refresh:\n{hovered_row}");
    }

    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.poll(Duration::from_millis(150));
    if harness.screen().contains_text("attach command") {
        anyhow::bail!("subagent action ran on mouse down");
    }
    harness.mouse_drag(column, 1)?;
    harness.mouse(MouseButton::Left, column, 1, false)?;
    harness.poll(Duration::from_millis(150));
    if harness.screen().contains_text("attach command") {
        anyhow::bail!("drag-away activated the subagent row");
    }

    let row = active_subagent_row(harness)?;
    harness.mouse_move(column, row)?;
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.poll(Duration::from_millis(100));
    if harness.screen().contains_text("attach command") {
        anyhow::bail!("subagent action ran before mouse release");
    }
    harness.mouse(MouseButton::Left, column, row, false)?;
    harness.wait_for_text(
        "attach command",
        WaitTimeout::secs(2, "subagent click release action"),
    )?;
    Ok(())
}

pub(super) const SUBAGENT_RAIL_MOUSE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_long_running_subagent"),
    Step::SubmitText("fixture subagent rail"),
    Step::WaitText {
        text: "subagent rail fixture dispatched",
        timeout: STREAM,
    },
    Step::Phase("hover_refresh_drag_and_click"),
    Step::Custom(assert_subagent_rail_mouse_flow),
    Step::ExitCommand,
];

pub(super) const SUBAGENT_RAIL_MOUSE_SCENARIO: Scenario = Scenario::new(
    "subagent_rail_mouse",
    "Keep hover through refreshes and activate rows on a completed click",
    DEFAULT_SIZE,
    SUBAGENT_RAIL_MOUSE_STEPS,
    /*smoke*/ false,
);
