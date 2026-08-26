//! Streamed markdown rendering scenarios.

use std::time::{Duration, Instant};

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

// Covers: already-drawn stream prose must stay visible through an open emphasis
// span and finish without leaking raw markers.
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
        text: "Stable prose",
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
    // Own the open-marker window: ALPHA must remain while the second delta is
    // visible and BETA has not arrived yet, then again through completion.
    let open_deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_open_window = false;
    while Instant::now() < open_deadline {
        harness.poll(Duration::from_millis(10));
        let screen = harness.screen().contents();
        if !screen.contains(ALPHA_MARKER) {
            anyhow::bail!(
                "stable stream prefix {ALPHA_MARKER} disappeared before emphasis closed:\n{screen}"
            );
        }
        if screen.contains(BETA_MARKER) {
            break;
        }
        if screen.contains(OPEN_EMPHASIS_WINDOW) {
            saw_open_window = true;
            // Sample again so a one-frame flash cannot pass.
            harness.poll(Duration::from_millis(20));
            let again = harness.screen().contents();
            if !again.contains(ALPHA_MARKER) {
                anyhow::bail!(
                    "stable stream prefix {ALPHA_MARKER} blanked during open emphasis:\n{again}"
                );
            }
            if again.contains(BETA_MARKER) {
                break;
            }
        }
    }
    if !saw_open_window {
        let screen = harness.screen().contents();
        // Fast CI can finish the stream before the poll loop observes the open
        // emphasis window. Accept a fully rendered completion when ALPHA,
        // the emphasis body, and BETA are already on screen without raw markers.
        if screen.contains(BETA_MARKER)
            && screen.contains(OPEN_EMPHASIS_WINDOW)
            && screen.contains(ALPHA_MARKER)
            && screen.contains(EMPHASIS_BODY)
            && !screen.contains("**")
        {
            return Ok(());
        }
        anyhow::bail!("never observed the open-emphasis window before {BETA_MARKER}\n{screen}");
    }

    let finish_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < finish_deadline {
        harness.poll(Duration::from_millis(30));
        let screen = harness.screen().contents();
        if !screen.contains(ALPHA_MARKER) {
            anyhow::bail!(
                "stable stream prefix {ALPHA_MARKER} disappeared before stream finished:\n{screen}"
            );
        }
        if screen.contains(BETA_MARKER) && screen.contains(EMPHASIS_BODY) {
            if screen.contains("**") {
                anyhow::bail!(
                    "raw emphasis markers leaked onto the finished stream screen:\n{screen}"
                );
            }
            return Ok(());
        }
    }
    anyhow::bail!(
        "stream never finished with {BETA_MARKER} and rendered emphasis body\n{}",
        harness.screen().contents()
    )
}
