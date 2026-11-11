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

fn active_subagent_row(harness: &mut PtyHarness) -> Result<u16> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        harness.pump(Duration::from_millis(20));
        if let Some(row) = harness.screen().rows_text().iter().position(|line| {
            line.contains("└ ◉ worker")
                || line.contains("├ ◉ worker")
                || line.contains("└ worker")
                || line.contains("├ worker")
        }) {
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

#[cfg(unix)]
fn wait_for_subagent_row(harness: &mut PtyHarness) -> Result<()> {
    active_subagent_row(harness).map(|_| ())
}

// Covers: a daemon session's subagent shows on the rail of the TUI that
// follows it, and `/attach` opens that run's transcript, though the run
// lives in the daemon and not in this process.
// Owner: interactive TUI as a daemon client
#[cfg(unix)]
pub(super) const DAEMON_SUBAGENT_RAIL_SCENARIO: Scenario = Scenario::new(
    "daemon_subagent_rail",
    "Show a daemon session's subagent on the rail and attach to it",
    DEFAULT_SIZE,
    &[
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
        Step::Custom(wait_for_subagent_row),
        Step::Phase("attach_from_picker"),
        Step::SubmitText("/attach"),
        Step::WaitText {
            text: "attach subagent",
            timeout: STREAM,
        },
        Step::Key(crate::keys::Key::Enter),
        Step::WaitText {
            text: "q back",
            timeout: STREAM,
        },
        Step::Key(crate::keys::Key::Esc),
        Step::WaitTextGone {
            text: "q back",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(super::daemon_resume::write_daemon_config)
.with_env(&[
    ("RHO_TUI_DAEMON", "1"),
    ("OPENAI_API_KEY", "sk-test-matrix"),
]);
