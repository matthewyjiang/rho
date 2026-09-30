//! Streamed markdown rendering scenarios.

use std::time::Duration;

use anyhow::Result;

use crate::{
    harness::{PtyHarness, WaitTimeout},
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

/// Markers must match `fixture markdown emphasis stream` in the matrix provider.
const ALPHA_MARKER: &str = "ALPHA";
const OPEN_EMPHASIS_WINDOW: &str = "while";
const BETA_MARKER: &str = "BETA";
const EMPHASIS_BODY: &str = "holding closes";

// Covers: streamed ATX headings render without retaining `#` markers.
// Owner: interactive TUI
const MARKDOWN_HEADINGS_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture markdown headings"),
    Step::WaitText {
        text: "Level six",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Custom(assert_markdown_headings_rendered),
    Step::ExitCommand,
];

// Covers: already-drawn prose stays visible through open emphasis, and closing
// inline Markdown must not commit a partial wrapped row before later text fills it.
// Owner: interactive TUI
const STREAMING_MARKDOWN_STABILITY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("stream_emphasis"),
    Step::SubmitText("fixture markdown emphasis stream"),
    Step::WaitText {
        text: ALPHA_MARKER,
        timeout: STREAM,
    },
    Step::Custom(assert_streaming_markdown_keeps_stable_prefix),
    Step::ExitCommand,
];

pub(super) const MARKDOWN_HEADINGS_SCENARIO: Scenario = Scenario::new(
    "markdown_headings",
    "Render streamed Markdown heading levels without syntax markers",
    SIZE,
    MARKDOWN_HEADINGS_STEPS,
    /* smoke */ false,
);

pub(super) const STREAMING_MARKDOWN_STABILITY_SCENARIO: Scenario = Scenario::new(
    "streaming_markdown_stability",
    "Keep already-drawn stream prose stable while later emphasis markers complete",
    SIZE,
    STREAMING_MARKDOWN_STABILITY_STEPS,
    /* smoke */ true,
);

fn assert_markdown_headings_rendered(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    for heading in [
        "Level one",
        "Level two",
        "Level three",
        "Level four",
        "Level five",
        "Level six",
    ] {
        if !screen.contains(heading) {
            anyhow::bail!("rendered heading is missing from the screen: {heading}");
        }
    }
    if screen
        .lines()
        .any(|line| line.trim_start().starts_with('#'))
    {
        anyhow::bail!("rendered heading retained Markdown syntax markers");
    }
    Ok(())
}

fn assert_streaming_markdown_keeps_stable_prefix(harness: &mut PtyHarness) -> Result<()> {
    // Release only after observing each durable checkpoint, never a timed window.
    for (phase, marker) in [
        ("open_emphasis", OPEN_EMPHASIS_WINDOW),
        ("closed_partial_row", "PARTIAL"),
        ("fill_partial_row", BETA_MARKER),
        ("completed_markdown", "Markdown stream complete"),
    ] {
        harness.set_phase(phase);
        super::fixture_release::release_fixture(harness, ".rho-fixture-release-markdown")?;
        harness.wait_for_text(marker, STREAM)?;
        let screen = harness.screen().contents();
        if !screen.contains(ALPHA_MARKER) {
            anyhow::bail!("stable stream prefix disappeared at {phase}:\n{screen}");
        }
        if marker != OPEN_EMPHASIS_WINDOW {
            if !screen.contains(EMPHASIS_BODY) || screen.contains("**") {
                anyhow::bail!("closed emphasis did not render at {phase}:\n{screen}");
            }
            if screen
                .lines()
                .any(|line| line.contains(ALPHA_MARKER) && line.contains("PARTIAL"))
            {
                anyhow::bail!("fixture did not wrap before its partial last row:\n{screen}");
            }
        }
        if matches!(marker, BETA_MARKER | "Markdown stream complete")
            && !screen
                .lines()
                .any(|line| line.contains("PARTIAL FILLS this same row BETA"))
        {
            anyhow::bail!(
                "later text split instead of filling the partial row at {phase}:\n{screen}"
            );
        }
    }
    Ok(())
}
