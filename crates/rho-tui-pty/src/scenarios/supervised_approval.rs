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
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Config · saves automatically",
        timeout: SETTLE,
    },
    Step::TypeText("agent"),
    Step::WaitText {
        text: "Agent behavior",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Permission mode"),
    Step::Key(Key::Enter),
    // Short terminals may only show the selected mode row; move onto
    // Supervised by its detail text instead of requiring every label to fit.
    Step::WaitText {
        text: "No permission checks",
        timeout: SETTLE,
    },
    Step::Custom(move_down_to_supervised_mode),
    Step::AssertText("Supervised"),
    Step::Key(Key::Enter),
    // Selection returns to the agent-behavior category with the label badge.
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Permission mode"),
    Step::AssertText("Supervised"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Appearance",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "permissions: supervised",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
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
    Step::WaitText {
        text: "fixture stream",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
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

/// Presses Down until the Supervised detail text is on screen, so the step
/// survives mode-list reordering: a literal press count re-encodes the
/// `PermissionMode::ALL` order and silently breaks when a mode lands ahead of
/// Supervised, which is exactly how #870 broke this scenario. Supervised is
/// the only mode whose detail contains the marker, so overshoot cannot
/// false-positive.
fn move_down_to_supervised_mode(harness: &mut PtyHarness) -> Result<()> {
    const MARKER: &str = "Ask before writes, processes, and outside-workspace reads";
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.poll(Duration::from_millis(30));
        if harness.screen().contains_text(MARKER) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "Supervised mode detail never became visible while pressing Down:\n{}",
                harness.screen().contents()
            );
        }
        harness.inject_key(&Key::Down)?;
        std::thread::sleep(Duration::from_millis(50));
    }
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
