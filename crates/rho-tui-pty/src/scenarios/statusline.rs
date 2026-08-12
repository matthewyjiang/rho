//! Statusline field hierarchy across terminal widths.

use std::time::Duration;

use anyhow::{ensure, Result};

use crate::{harness::PtyHarness, scenario::Step};

use super::{SETTLE, STARTUP};

pub(super) const STATUSLINE_HIERARCHY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "Bypass",
        timeout: STARTUP,
    },
    Step::Custom(assert_wide_hierarchy),
    Step::Phase("drop_provider"),
    // Wide enough for permission + model, too narrow for the OpenAI label.
    Step::Resize { rows: 28, cols: 20 },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Custom(assert_provider_dropped_model_kept),
    Step::Phase("keep_permission"),
    // Bare permission must remain after the model no longer fits.
    Step::Resize { rows: 28, cols: 10 },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Custom(assert_permission_kept),
    Step::ExitCommand,
];

fn bottom_status_row(harness: &PtyHarness) -> Result<String> {
    let rows = harness.screen().rows_text();
    rows.iter()
        .rev()
        .find(|row| {
            let trimmed = row.trim();
            !trimmed.is_empty()
                && (trimmed.contains("Bypass")
                    || trimmed.contains("gpt-5.5")
                    || trimmed.contains("OpenAI"))
        })
        .map(|row| row.trim().to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "statusline bottom row not found:\n{}",
                harness.screen().debug_dump()
            )
        })
}

fn assert_wide_hierarchy(harness: &mut PtyHarness) -> Result<()> {
    let row = bottom_status_row(harness)?;
    ensure!(
        row.contains("Bypass") && row.contains("OpenAI") && row.contains("gpt-5.5"),
        "wide statusline lost the ranked identity fields:\n{row}\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

fn assert_provider_dropped_model_kept(harness: &mut PtyHarness) -> Result<()> {
    let row = bottom_status_row(harness)?;
    ensure!(
        row.contains("Bypass") && row.contains("gpt-5.5"),
        "medium statusline should keep permission and model:\n{row}\n{}",
        harness.screen().debug_dump()
    );
    ensure!(
        !row.contains("OpenAI"),
        "provider must drop before the model:\n{row}\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

fn assert_permission_kept(harness: &mut PtyHarness) -> Result<()> {
    let row = bottom_status_row(harness)?;
    ensure!(
        row.contains("Bypass"),
        "narrow statusline should keep permission:\n{row}\n{}",
        harness.screen().debug_dump()
    );
    ensure!(
        !row.contains("gpt-5.5") && !row.contains("OpenAI"),
        "model and provider must drop before permission:\n{row}\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}
