use std::time::Duration;

use anyhow::{ensure, Result};

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
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Custom(assert_runtime_info_stacked),
    Step::ExitCommand,
];

fn assert_runtime_info_stacked(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let permissions_row = rows.iter().position(|row| row.trim() == "Permissions");
    let stacked = permissions_row
        .and_then(|index| rows.get(index + 1))
        .is_some_and(|row| row.trim() == "auto");
    ensure!(
        stacked,
        "runtime info did not stack the Permissions field after resize:\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}
