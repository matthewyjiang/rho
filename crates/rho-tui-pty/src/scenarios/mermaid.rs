use std::time::Duration;

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
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Custom(assert_diagram_replaced_source),
    Step::Phase("narrow_pane"),
    Step::Resize { rows: 40, cols: 44 },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Custom(assert_narrow_pane_explains_fallback),
    Step::Phase("restore_pane"),
    Step::Resize {
        rows: 28,
        cols: 100,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Custom(assert_diagram_replaced_source),
    Step::ExitCommand,
];

fn assert_diagram_replaced_source(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    ensure!(
        !screen.contains("flowchart LR"),
        "closed mermaid fence kept its source:\n{}",
        harness.screen().debug_dump()
    );
    for phase in ["Phase 1", "Phase 5"] {
        ensure!(
            screen.contains(phase),
            "rendered diagram is missing {phase}:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
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
    Ok(())
}
