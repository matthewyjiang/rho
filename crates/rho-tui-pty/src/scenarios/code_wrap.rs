//! Exact-width fenced-code graphemes must survive the full transcript renderer.

use anyhow::{ensure, Context, Result};

use crate::{
    harness::PtyHarness,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::STARTUP;

// Covers: hard wrapping must not detach an accent from a base that fills a row.
// Owner: interactive TUI; pure wrap tests do not observe the rendered accent.
pub(super) const CODE_WRAP_GRAPHEME_SCENARIO: Scenario = Scenario::new(
    "code_wrap_grapheme",
    "Preserve combining accents at exact-width fenced-code wrap boundaries",
    // Issue #1345: 40 terminal columns leave 38 transcript content columns.
    PtySize { rows: 24, cols: 40 },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(assert_code_wrap_grapheme),
        Step::ExitCommand,
    ],
    /* smoke */ true,
);

fn assert_code_wrap_grapheme(harness: &mut PtyHarness) -> Result<()> {
    let first_row = format!("{}e\u{301}", "a".repeat(37));
    let marker = "code wrap complete";
    // Keep the opening fence on its own line after the fixture's echo prefix.
    harness.submit_text(&format!(
        "code wrap\n```\n{first_row}z\n```\ncode **wrap** complete"
    ))?;
    // Only assistant rendering removes the emphasis markers; the submitted prompt
    // cannot satisfy this wait. The marker follows both completed code rows.
    harness.wait_for_text(marker, STARTUP)?;
    let screen = harness.screen().contents();
    let (_, response) = screen
        .split_once("fixture response:")
        .context("assistant echo prefix missing")?;
    let lines: Vec<_> = response.lines().collect();
    let first = lines
        .iter()
        .position(|line| line.trim() == first_row)
        .with_context(|| format!("intact accented code row missing:\n{screen}"))?;
    ensure!(
        lines.get(first + 1).map(|line| line.trim()) == Some("z"),
        "code continuation must occupy the next row:\n{screen}"
    );
    Ok(())
}
