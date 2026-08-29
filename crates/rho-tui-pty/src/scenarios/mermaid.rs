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
    Step::Phase("stream_diagram_live"),
    Step::SubmitText("fixture mermaid flowchart"),
    // Art must appear from complete-line prefixes before the closing fence.
    Step::Custom(wait_until_live_streamed_diagram),
    Step::Phase("stream_diagram"),
    Step::WaitText {
        text: "diagram delivered",
        timeout: STREAM,
    },
    Step::Custom(wait_until_diagram_art),
    Step::Phase("split_pane"),
    Step::Resize { rows: 40, cols: 44 },
    // A typical split pane used to dump this LR chain as source. It should now
    // relayout as TD art instead of showing a fallback title.
    Step::Custom(wait_until_diagram_art),
    Step::Phase("narrow_pane"),
    Step::Resize { rows: 40, cols: 32 },
    // 32 columns cannot hold even the TD relayout, but instead of a source
    // dump the diagram now clips at the right edge under a CLIPPED title with
    // a marker row counting the hidden columns.
    Step::WaitText {
        text: "cols clipped",
        timeout: SETTLE,
    },
    Step::Custom(assert_narrow_pane_clips_art),
    Step::Phase("tiny_pane"),
    Step::Resize { rows: 40, cols: 20 },
    // Below the clip floor (~24 inner columns) the source fallback remains.
    // The panel header truncates at this width, so wait on the source itself.
    Step::WaitText {
        text: "flowchart LR",
        timeout: SETTLE,
    },
    Step::Custom(assert_tiny_pane_falls_back_to_source),
    Step::Phase("restore_pane"),
    Step::Resize {
        rows: 28,
        cols: 100,
    },
    Step::Custom(wait_until_diagram_art),
    Step::ExitCommand,
];

fn wait_until_live_streamed_diagram(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + STREAM.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        let screen = harness.screen().contents();
        if screen.contains("diagram delivered") {
            anyhow::bail!(
                "closing fence arrived before live mermaid art:\n{}",
                harness.screen().debug_dump()
            );
        }
        if diagram_art_visible(harness) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for live mermaid art before the closing fence:\n{}",
                harness.screen().debug_dump()
            );
        }
    }
}

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
        && !screen.contains("PANE TOO")
        && screen.contains("Phase 1")
        && screen.contains("Phase 5")
}

fn assert_narrow_pane_clips_art(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    ensure!(
        screen.contains("CLIPPED"),
        "narrow pane did not use the clipped title:\n{}",
        harness.screen().debug_dump()
    );
    ensure!(
        !screen.contains("flowchart LR"),
        "narrow pane dumped source instead of clipped art:\n{}",
        harness.screen().debug_dump()
    );
    ensure!(
        screen.contains("┌") || screen.contains("╭"),
        "narrow pane lost the diagram art:\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

fn assert_tiny_pane_falls_back_to_source(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    ensure!(
        screen.contains("flowchart LR"),
        "tiny pane fallback dropped the mermaid source:\n{}",
        harness.screen().debug_dump()
    );
    ensure!(
        !screen.contains("┌") && !screen.contains("╭"),
        "tiny pane still showed diagram art or block borders:\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}
