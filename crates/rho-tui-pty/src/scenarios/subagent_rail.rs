//! Subagent activity-rail pointer behavior.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::{
    assert_helpers::{assert_cancelled_rail_click, mouse_row_text},
    DEFAULT_SIZE, STARTUP, STREAM,
};
use crate::{
    harness::WaitTimeout,
    keys::MouseButton,
    scenario::{Scenario, Step},
    PtyHarness,
};

/// Whether `line` is the rail row of a subagent running as `role`. The
/// agent tool's card names the role too, before the rail lists the run.
fn is_rail_row(line: &str, role: &str) -> bool {
    ["└ ◉ ", "├ ◉ ", "└ ", "├ "]
        .iter()
        .any(|branch| line.contains(&format!("{branch}{role}")))
}

/// Waits until the rail lists a subagent running as `role`. `/attach`
/// offers only runs the rail lists, and a daemon session's rail learns of a
/// run from the daemon's jobs, after the card that started it shows.
fn wait_for_rail_row(harness: &mut PtyHarness, role: &'static str) -> Result<()> {
    harness.wait_for_screen("the subagent's rail row", super::SETTLE, |screen| {
        screen
            .rows_text()
            .iter()
            .any(|line| is_rail_row(line, role))
    })
}

pub(super) fn wait_for_worker_row(harness: &mut PtyHarness) -> Result<()> {
    wait_for_rail_row(harness, "worker")
}

pub(super) fn wait_for_explorer_row(harness: &mut PtyHarness) -> Result<()> {
    wait_for_rail_row(harness, "explorer")
}

fn active_subagent_row(harness: &mut PtyHarness) -> Result<u16> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.pump(Duration::from_millis(20));
        if let Some(row) = harness
            .screen()
            .rows_text()
            .iter()
            .position(|line| is_rail_row(line, "worker"))
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
    harness.wait_for_text("attach", WaitTimeout::secs(2, "subagent hover action hint"))?;

    // A hovered row ends in `⏎ attach · <elapsed>`, and only a snapshot
    // refresh advances the elapsed seconds; the hover hint must survive it.
    let elapsed = |line: &str| line.rsplit("· ").next().unwrap_or_default().to_owned();
    let hovered = elapsed(&mouse_row_text(harness.screen(), row));
    harness.wait_for_screen(
        "subagent rail snapshot refresh",
        WaitTimeout::secs(3, "subagent elapsed tick"),
        |screen| elapsed(&mouse_row_text(screen, row)) != hovered,
    )?;
    let refreshed = mouse_row_text(harness.screen(), row);
    if !refreshed.contains("attach") {
        anyhow::bail!("hover disappeared after snapshot refresh:\n{refreshed}");
    }

    assert_cancelled_rail_click(harness, column, row, "attach")?;

    let row = active_subagent_row(harness)?;
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.mouse(MouseButton::Left, column, row, false)?;
    harness.wait_for_text(
        "q back",
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
