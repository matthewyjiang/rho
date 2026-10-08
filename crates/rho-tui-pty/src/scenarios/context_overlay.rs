//! `/context` overlay scenario.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

const SIZE: PtySize = PtySize {
    rows: 40,
    cols: 110,
};

// Covers: /context reads the live session after a tool turn, attributes the
// tool's output to that tool, keeps right-aligned token and percent columns
// inside the panel, reports free space against the window rather than the
// used total, stacks numbers under labels when narrow instead of blanking rows,
// and closes without leaving a transcript block.
// Owner: interactive TUI command dispatch and panel rendering; grouping policy
// is unit-tested beside `ContextReport`.
const CONTEXT_OVERLAY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("tool_turn"),
    Step::SubmitText("fixture tool"),
    Step::WaitText {
        text: "Worked for",
        timeout: STREAM,
    },
    Step::Phase("open_context"),
    Step::SubmitText("/context"),
    Step::WaitText {
        text: "Tool results",
        timeout: SETTLE,
    },
    Step::Custom(assert_context_overlay),
    Step::Phase("narrow"),
    Step::Resize { rows: 60, cols: 30 },
    Step::Custom(wait_until_numbers_stack),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Tool results",
        timeout: SETTLE,
    },
    Step::Custom(assert_context_dismissed),
    Step::ExitCommand,
];

pub(super) const CONTEXT_OVERLAY_SCENARIO: Scenario = Scenario::new(
    "context_overlay",
    "Run a tool turn, attribute the context window by source in /context, and dismiss",
    SIZE,
    CONTEXT_OVERLAY_STEPS,
    /* smoke */ false,
);

fn assert_context_overlay(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_hidden_cursor(SETTLE)?;
    let screen = harness.screen().contents();
    for needle in [" Context ", "System prompt", "Tool schemas", "Messages"] {
        if !screen.contains(needle) {
            bail!("context overlay missing {needle:?}:\n{screen}");
        }
    }
    let rows: Vec<String> = harness
        .screen()
        .rows_text()
        .iter()
        .map(|row| panel_cell(row))
        .collect();
    let Some(results) = rows.iter().position(|row| row.starts_with("Tool results")) else {
        bail!("tool results group missing:\n{screen}");
    };
    // The fixture turn calls `write`; its result must be attributed to it.
    if !rows
        .get(results + 1)
        .is_some_and(|row| row.starts_with("write "))
    {
        bail!("write result not grouped under Tool results:\n{screen}");
    }
    for row in &rows[results..=results + 1] {
        if !row.ends_with('%') {
            bail!("token and percent columns clipped in {row:?}:\n{screen}");
        }
    }
    // Free space is a share of the window, so it can never exceed 100%.
    let free_percent = rows
        .iter()
        .find_map(|row| row.strip_prefix("Free space "))
        .and_then(|row| {
            row.rsplit(' ')
                .next()?
                .strip_suffix('%')?
                .parse::<f64>()
                .ok()
        });
    if !free_percent.is_some_and(|percent| percent <= 100.0) {
        bail!("free space row missing or not a share of the window:\n{screen}");
    }
    Ok(())
}

/// Too narrow for side-by-side columns: each label keeps its own line and its
/// token count and percent move to the next line instead of disappearing.
fn wait_until_numbers_stack(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + SETTLE.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        let rows: Vec<String> = harness
            .screen()
            .rows_text()
            .iter()
            .map(|row| panel_cell(row))
            .collect();
        let stacked = rows
            .iter()
            .position(|row| row == "Tool results")
            .and_then(|index| rows.get(index + 1))
            .is_some_and(|row| row.ends_with('%'));
        if stacked {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "narrow /context did not stack numbers under labels:\n{}",
                harness.screen().debug_dump()
            );
        }
    }
}

fn assert_context_dismissed(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_visible_cursor(SETTLE)?;
    let screen = harness.screen().contents();
    if screen.contains(" Context ") || screen.contains("Tool schemas") {
        bail!("context overlay or its report stayed visible after Esc:\n{screen}");
    }
    Ok(())
}

/// Drop overlay borders and the scrollbar so a row compares as plain text.
fn panel_cell(row: &str) -> String {
    row.chars()
        .filter(|ch| !matches!(ch, '│' | '█' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '─'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
