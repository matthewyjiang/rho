//! Live background-process rows in the activity rail.

use std::time::{Duration, Instant};

use anyhow::Result;

use super::{DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    harness::WaitTimeout,
    keys::{Key, MouseButton},
    scenario::{Scenario, Step},
    PtyHarness,
};

pub(super) const PROCESS_RAIL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_process"),
    Step::SubmitText("fixture process rail"),
    Step::WaitText {
        text: "process rail fixture dispatched",
        timeout: STREAM,
    },
    Step::Phase("rail_survives_turn_end"),
    Step::WaitText {
        text: "└ ⚙ sleep 60",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

fn active_process_row(harness: &mut PtyHarness) -> Result<u16> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        if let Some(row) = harness.screen().rows_text().iter().position(|line| {
            line.contains("└ ⚙ sleep 60")
                || line.contains("├ ⚙ sleep 60")
                || line.contains("└ sleep 60")
                || line.contains("├ sleep 60")
        }) {
            return Ok(row as u16 + 1);
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "active process row did not appear:\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn assert_process_rail_peek_flow(harness: &mut PtyHarness) -> Result<()> {
    let column = 3;
    let row = active_process_row(harness)?;
    harness.mouse_move(column, row)?;
    harness.wait_for_text("peek", WaitTimeout::secs(2, "process hover action hint"))?;

    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.poll(Duration::from_millis(150));
    if harness.screen().contains_text("q back") {
        anyhow::bail!("process peek ran on mouse down");
    }
    harness.mouse_drag(column, 1)?;
    harness.mouse(MouseButton::Left, column, 1, false)?;
    harness.poll(Duration::from_millis(150));
    if harness.screen().contains_text("q back") {
        anyhow::bail!("drag-away activated the process row");
    }

    let row = active_process_row(harness)?;
    harness.mouse_move(column, row)?;
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.poll(Duration::from_millis(100));
    if harness.screen().contains_text("q back") {
        anyhow::bail!("process peek ran before mouse release");
    }
    harness.mouse(MouseButton::Left, column, row, false)?;
    harness.wait_for_text("q back", WaitTimeout::secs(2, "process click release peek"))?;
    if !harness.screen().contains_text("sleep 60") {
        anyhow::bail!(
            "peek header missing command:\n{}",
            harness.screen().debug_dump()
        );
    }
    harness.inject_key(&Key::Char('q'))?;
    harness.wait_for_text(
        "└ ⚙ sleep 60",
        WaitTimeout::secs(2, "peek q returns to composer"),
    )?;
    Ok(())
}

pub(super) const PROCESS_RAIL_PEEK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_process"),
    Step::SubmitText("fixture process rail"),
    Step::WaitText {
        text: "process rail fixture dispatched",
        timeout: STREAM,
    },
    Step::Phase("hover_drag_and_click_peek"),
    Step::WaitText {
        text: "└ ⚙ sleep 60",
        timeout: STREAM,
    },
    Step::Custom(assert_process_rail_peek_flow),
    Step::ExitCommand,
];

/// Queued input must render *below* live activity rows.
///
/// A follow-up is queued while a background process is still live, so the
/// process rail and the pending-input panel are on screen together. Asserting
/// their row order is what fails if pending input ever moves back above active
/// work: with the pre-fix layout the pending row sat above the process row.
fn assert_pending_input_below_process_row(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.poll(Duration::from_millis(20));
        let rows = harness.screen().rows_text();
        let process_row = rows.iter().position(|line| line.contains("⚙ sleep 60"));
        // The queued follow-up renders as a NEXT row in the pending-input panel.
        let pending_row = rows.iter().position(|line| line.contains("NEXT"));
        if let (Some(process_row), Some(pending_row)) = (process_row, pending_row) {
            if process_row < pending_row {
                return Ok(());
            }
            anyhow::bail!(
                "pending input (row {pending_row}) must render below the live process row \
                 (row {process_row}):\n{}",
                harness.screen().debug_dump()
            );
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "needed a live process row and a queued follow-up on screen together, got \
                 process={process_row:?} pending={pending_row:?}:\n{}",
                harness.screen().debug_dump()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `sleep 60` outlives the turn that started it, so the second turn has a live
/// process row on screen while a follow-up is queued into it.
pub(super) const PENDING_INPUT_BELOW_ACTIVITY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_process"),
    Step::SubmitText("fixture process rail"),
    Step::WaitText {
        text: "process rail fixture dispatched",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "└ ⚙ sleep 60",
        timeout: STREAM,
    },
    Step::Phase("start_second_turn"),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::Phase("queue_follow_up_while_process_live"),
    Step::TypeText("queued follow up"),
    Step::Key(Key::AltEnter),
    Step::WaitText {
        text: "1 follow-up",
        timeout: STREAM,
    },
    Step::Custom(assert_pending_input_below_process_row),
    Step::CtrlCExit,
];

pub(super) const PENDING_INPUT_BELOW_ACTIVITY_SCENARIO: Scenario = Scenario::new(
    "pending_input_below_activity",
    "Queue a follow-up while a process is live and keep pending input below the rail",
    DEFAULT_SIZE,
    PENDING_INPUT_BELOW_ACTIVITY_STEPS,
    /*smoke*/ true,
);

pub(super) const PROCESS_RAIL_SCENARIO: Scenario = Scenario::new(
    "process_rail",
    "Show a live background process in the activity rail after the turn ends",
    DEFAULT_SIZE,
    PROCESS_RAIL_STEPS,
    /*smoke*/ false,
);

pub(super) const PROCESS_RAIL_PEEK_SCENARIO: Scenario = Scenario::new(
    "process_rail_peek",
    "Hover a process row, then click to open and leave the read-only peek view",
    DEFAULT_SIZE,
    PROCESS_RAIL_PEEK_STEPS,
    /*smoke*/ false,
);
