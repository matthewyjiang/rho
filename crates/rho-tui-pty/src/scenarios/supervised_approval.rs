//! Supervised permission-mode switch and bounded process approval.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    scenario::Step,
};

use super::{SETTLE, STARTUP};

const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");

pub(super) const SUPERVISED_APPROVAL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("enable_supervised_mode"),
    Step::SubmitText("/permissions supervised"),
    // Mode changes are routine status text without a toast; the statusline
    // segment is the observable signal.
    Step::WaitText {
        text: "Supervised ·",
        timeout: SETTLE,
    },
    Step::Phase("inspect_long_process_approval"),
    Step::SubmitText("fixture approval long"),
    Step::WaitText {
        text: "bash wants to run a command",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "reviewing harmless fixture",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Allow for session",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "→ Deny",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "PgUp/PgDn details",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "↓ later",
        timeout: SETTLE,
    },
    // Page size depends on terminal chrome; scroll until the suffix is visible
    // instead of hard-coding a PageDown count.
    Step::Custom(scroll_approval_detail_until_suffix_visible),
    Step::WaitText {
        text: "↑ earlier",
        timeout: SETTLE,
    },
    Step::Key(Key::PageUp),
    Step::WaitText {
        text: "reviewing harmless fixture",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    // Esc always denies the pending call, but whether the turn is also
    // interrupted races the instant fixture response. Each outcome renders
    // different text, so accept either durable form.
    Step::Custom(wait_for_denied_or_interrupted),
    // Abort teardown can restore composer text after Esc. Wait until the
    // session is quiet and the input prompt is empty, then type the follow-up
    // instead of pasting so Enter cannot land in the dying turn.
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Type a message",
        timeout: SETTLE,
    },
    Step::Phase("continue_session"),
    Step::TypeText("fixture stream"),
    Step::Custom(submit_typed_follow_up),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "part two",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

/// Typed text runs through the paste-burst detector, whose Enter suppression
/// window starts when Rho *reads* the last key. A stalled runner can read the
/// tail of the text in one turn, so Enter sent right after it renders becomes
/// a newline. Wait for the text (Rho has read every key), then settle.
fn submit_typed_follow_up(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("fixture stream", SETTLE)?;
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Enter)
}

fn wait_for_denied_or_interrupted(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        harness.poll(Duration::from_millis(30));
        let screen = harness.screen();
        if screen.contains_text("model interrupted")
            || screen.contains_text("capability denied: cancelled by user")
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "approval Esc produced neither an interrupt nor a denial:\n{}",
                harness.screen().contents()
            );
        }
    }
}

fn scroll_approval_detail_until_suffix_visible(harness: &mut PtyHarness) -> Result<()> {
    const MARKER: &str = "DANGEROUS_SUFFIX_INSPECTABLE";
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        harness.poll(Duration::from_millis(30));
        if harness.screen().contains_text(MARKER) {
            return Ok(());
        }
        harness.inject_key(&Key::PageDown)?;
        std::thread::sleep(Duration::from_millis(50));
    }
    harness.poll(Duration::from_millis(50));
    if harness.screen().contains_text(MARKER) {
        return Ok(());
    }
    anyhow::bail!(
        "approval detail suffix never became visible after PageDown scrolling\n{}",
        harness.screen().contents()
    )
}
