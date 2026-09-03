use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::{harness::PtyHarness, scenario::Step};

use super::{SETTLE, STARTUP};

pub(super) const RUNTIME_INFO_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "rho",
        timeout: STARTUP,
    },
    Step::Phase("open_info"),
    Step::SubmitText("/info"),
    Step::WaitText {
        text: "Model",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Session usage",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Workspace",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Permissions",
        timeout: SETTLE,
    },
    Step::Resize { rows: 44, cols: 30 },
    // Poll for the stacked layout instead of waiting for output to go quiet:
    // a loaded runner can leave the screen blank between the clear and the
    // redraw long enough for a quiet window to pass.
    Step::Custom(wait_until_runtime_info_stacked),
    Step::ExitCommand,
];

fn wait_until_runtime_info_stacked(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + SETTLE.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        if runtime_info_stacked(harness) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "runtime info did not stack the Permissions field after resize:\n{}",
                harness.screen().debug_dump()
            );
        }
    }
}

fn runtime_info_stacked(harness: &PtyHarness) -> bool {
    let rows = harness.screen().rows_text();
    rows.iter()
        .position(|row| row.trim() == "Permissions")
        .and_then(|index| rows.get(index + 1))
        .is_some_and(|row| row.trim() == "bypass")
}
