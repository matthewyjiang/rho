use std::time::{Duration, Instant};

use anyhow::{ensure, Result};

use crate::{harness::PtyHarness, scenario::Step};

use super::{SETTLE, STARTUP, STREAM};

pub(super) const MERMAID_FLOWCHART_RESIZE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("stream_diagram"),
    Step::SubmitText("fixture mermaid flowchart"),
    Step::WaitText {
        text: "flowchart LR",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "diagram delivered",
        timeout: STREAM,
    },
    Step::Custom(wait_until_diagram_art),
    Step::Phase("narrow_pane"),
    Step::Resize { rows: 40, cols: 44 },
    // Wait for the reflowed fallback, not merely a quiet frame. Resize can leave
    // the previous wide art clipped until history rebuilds at the new width.
    Step::WaitText {
        text: "PANE TOO NARROW",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "flowchart LR",
        timeout: SETTLE,
    },
    Step::Custom(assert_narrow_pane_explains_fallback),
    Step::Phase("restore_pane"),
    Step::Resize {
        rows: 28,
        cols: 100,
    },
    Step::Custom(wait_until_diagram_art),
    Step::ExitCommand,
];

fn wait_until_diagram_art(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + SETTLE.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        if diagram_art_visible(harness) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for rendered mermaid art:\n{}",
                harness.screen().debug_dump()
            );
        }
    }
}

fn diagram_art_visible(harness: &PtyHarness) -> bool {
    let screen = harness.screen().contents();
    !screen.contains("flowchart LR")
        && !screen.contains("PANE TOO NARROW")
        && screen.contains("Phase 1")
        && screen.contains("Phase 5")
}

fn assert_narrow_pane_explains_fallback(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    ensure!(
        screen.contains("PANE TOO NARROW"),
        "narrow pane fallback did not explain itself:\n{}",
        harness.screen().debug_dump()
    );
    ensure!(
        screen.contains("flowchart LR"),
        "narrow pane fallback dropped the mermaid source:\n{}",
        harness.screen().debug_dump()
    );
    ensure!(
        !screen.contains("┌") && !screen.contains("╭─ MERMAID ─"),
        "narrow pane still showed diagram art instead of source:\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}
