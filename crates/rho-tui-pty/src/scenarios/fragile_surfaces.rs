//! PTY coverage for historically fragile interactive surfaces.
//!
//! Streaming markdown rewrite, activity-rail anchoring, help, slash palette,
//! and `@` file autocomplete all regressed without screen-level checks.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{
    env::IsolatedHome,
    harness::{PtyHarness, WaitTimeout},
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

const ALPHA_MARKER: &str = "ALPHA";
const BETA_MARKER: &str = "BETA";
const STABLE_PHRASE: &str = "Stable prose ALPHA remains drawn";
const EMPHASIS_BODY: &str = "holding closes";
const FILE_ALPHA: &str = "alpha-unique-fixture.txt";
const FILE_BETA: &str = "beta-unique-fixture.txt";

/// Activity phase labels that can appear on the rail during a live turn.
const ACTIVITY_LABELS: &[&str] = &[
    "responding",
    "thinking",
    "waiting for provider",
    "starting",
    "running tool",
    "preparing tool",
];

// Covers: already-drawn stream prose must stay on screen while later emphasis
// markers complete; raw markdown markers must not leak after the span closes.
// Owner: interactive TUI
pub(super) const STREAMING_MARKDOWN_STABILITY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("stream_emphasis"),
    Step::SubmitText("fixture markdown emphasis stream"),
    Step::WaitText {
        text: STABLE_PHRASE,
        timeout: STREAM,
    },
    Step::Custom(assert_streaming_markdown_keeps_stable_prefix),
    Step::ExitCommand,
];

// Covers: the activity rail must sit above the composer during a live turn, and
// the jump control must share that rail when the user scrolls away from bottom.
// Owner: interactive TUI
pub(super) const SPINNER_ACTIVITY_ANCHOR_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("seed_history"),
    Step::SubmitText("fixture bulk one"),
    Step::WaitText {
        text: "fixture bulk one line 180",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Phase("start_turn"),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "responding",
        timeout: STREAM,
    },
    Step::Custom(assert_activity_anchored_above_composer),
    Step::Phase("scroll_away"),
    Step::Key(Key::PageUp),
    Step::Key(Key::PageUp),
    Step::Key(Key::PageUp),
    Step::WaitText {
        text: "bottom",
        timeout: WaitTimeout::secs(5, "jump control while scrolled"),
    },
    Step::Custom(assert_activity_shares_rail_with_jump),
    Step::Phase("cancel"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

// Covers: /help must open the shortcuts overlay and Esc must return to the
// session without leaving a stuck picker.
// Owner: interactive TUI
pub(super) const HELP_OVERLAY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_help"),
    Step::SubmitText("/help"),
    Step::WaitText {
        text: "Keyboard shortcuts",
        timeout: SETTLE,
    },
    Step::AssertText("KEYS"),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Custom(assert_help_overlay_dismissed),
    Step::ExitCommand,
];

// Covers: typing / must open the command palette; filtering must narrow to the
// matching command surface.
// Owner: interactive TUI
pub(super) const SLASH_COMMAND_PALETTE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_palette"),
    Step::TypeText("/"),
    // The palette shows a short top slice in name order; /agents is first.
    Step::WaitText {
        text: "/agents",
        timeout: SETTLE,
    },
    Step::AssertText("/config"),
    Step::Phase("filter"),
    Step::TypeText("mod"),
    Step::WaitText {
        text: "/model",
        timeout: SETTLE,
    },
    Step::Custom(assert_slash_palette_filtered_to_model),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) fn setup_file_autocomplete(home: &IsolatedHome) -> Result<()> {
    std::fs::write(home.workspace.join(FILE_ALPHA), "alpha fixture body\n")?;
    std::fs::write(home.workspace.join(FILE_BETA), "beta fixture body\n")?;
    Ok(())
}

// Covers: @ must open workspace path autocomplete and Enter must insert the
// selected path into the composer.
// Owner: interactive TUI
pub(super) const FILE_PATH_AUTOCOMPLETE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_file_palette"),
    Step::TypeText("@alpha"),
    Step::WaitText {
        text: FILE_ALPHA,
        timeout: SETTLE,
    },
    Step::Custom(assert_file_palette_filtered_to_alpha),
    Step::Phase("select"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "file path inserted",
        timeout: SETTLE,
    },
    Step::Custom(assert_file_path_inserted),
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
];

pub(super) const STREAMING_MARKDOWN_STABILITY_SCENARIO: Scenario = Scenario::new(
    "streaming_markdown_stability",
    "Keep already-drawn stream prose stable while later emphasis markers complete",
    SIZE,
    STREAMING_MARKDOWN_STABILITY_STEPS,
    /* smoke */ true,
);

pub(super) const SPINNER_ACTIVITY_ANCHOR_SCENARIO: Scenario = Scenario::new(
    "spinner_activity_anchor",
    "Keep the activity rail above the composer and share it with jump-to-bottom",
    SIZE,
    SPINNER_ACTIVITY_ANCHOR_STEPS,
    /* smoke */ true,
);

pub(super) const HELP_OVERLAY_SCENARIO: Scenario = Scenario::new(
    "help_overlay",
    "Open the keyboard shortcuts overlay and dismiss it cleanly",
    SIZE,
    HELP_OVERLAY_STEPS,
    /* smoke */ true,
);

pub(super) const SLASH_COMMAND_PALETTE_SCENARIO: Scenario = Scenario::new(
    "slash_command_palette",
    "Open the slash command palette and filter to a matching command",
    SIZE,
    SLASH_COMMAND_PALETTE_STEPS,
    /* smoke */ false,
);

pub(super) const FILE_PATH_AUTOCOMPLETE_SCENARIO: Scenario = Scenario {
    id: "file_path_autocomplete",
    description: "Open @ path autocomplete and insert a workspace file reference",
    size: SIZE,
    setup: Some(setup_file_autocomplete),
    env: &[],
    steps: FILE_PATH_AUTOCOMPLETE_STEPS,
    smoke: false,
};

fn assert_streaming_markdown_keeps_stable_prefix(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_beta = false;
    while Instant::now() < deadline {
        harness.poll(Duration::from_millis(30));
        let screen = harness.screen().contents();
        if !screen.contains(ALPHA_MARKER) {
            anyhow::bail!(
                "stable stream prefix {ALPHA_MARKER} disappeared while emphasis was still open:\n{screen}"
            );
        }
        if screen.contains(BETA_MARKER) {
            saw_beta = true;
            break;
        }
    }
    if !saw_beta {
        anyhow::bail!(
            "stream never reached trailing {BETA_MARKER} marker\n{}",
            harness.screen().contents()
        );
    }

    harness.poll(Duration::from_millis(50));
    let screen = harness.screen().contents();
    if !screen.contains(ALPHA_MARKER) {
        anyhow::bail!(
            "stable stream prefix {ALPHA_MARKER} missing after stream finished:\n{screen}"
        );
    }
    if !screen.contains(BETA_MARKER) {
        anyhow::bail!(
            "trailing stream marker {BETA_MARKER} missing after stream finished:\n{screen}"
        );
    }
    if !screen.contains(EMPHASIS_BODY) {
        anyhow::bail!("completed emphasis body missing after stream finished:\n{screen}");
    }
    if screen.contains("**") {
        anyhow::bail!("raw emphasis markers leaked onto the finished stream screen:\n{screen}");
    }
    Ok(())
}

fn row_with_any(rows: &[String], needles: &[&str]) -> Option<usize> {
    rows.iter()
        .position(|row| needles.iter().any(|needle| row.contains(needle)))
}

fn assert_activity_anchored_above_composer(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let activity_row = row_with_any(&rows, ACTIVITY_LABELS).ok_or_else(|| {
        anyhow::anyhow!(
            "activity rail label missing while turn is live:\n{}",
            harness.screen().contents()
        )
    })?;
    let composer_row = rows
        .iter()
        .position(|row| {
            let trimmed = row.trim_start();
            trimmed.starts_with("> ") || trimmed == ">" || row.contains("Type a message")
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "composer prompt missing while turn is live:\n{}",
                harness.screen().contents()
            )
        })?;
    if activity_row >= composer_row {
        anyhow::bail!(
            "activity rail row {activity_row} is not above composer row {composer_row}:\n{}",
            harness.screen().contents()
        );
    }
    // Bottom-follow keeps a breathing gap so transcript content is not drawn
    // on the activity row itself.
    if rows[activity_row].contains("partial assistant before cancellation") {
        anyhow::bail!(
            "transcript content occupied the activity rail row:\n{}",
            harness.screen().contents()
        );
    }
    Ok(())
}

fn assert_activity_shares_rail_with_jump(harness: &mut PtyHarness) -> Result<()> {
    let rows = harness.screen().rows_text();
    let activity_row = row_with_any(&rows, ACTIVITY_LABELS).ok_or_else(|| {
        anyhow::anyhow!(
            "activity rail label missing after scroll:\n{}",
            harness.screen().contents()
        )
    })?;
    let jump_row = rows
        .iter()
        .position(|row| row.contains("bottom") && row.contains('↓'))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "jump-to-bottom control missing after scroll:\n{}",
                harness.screen().contents()
            )
        })?;
    if activity_row != jump_row {
        anyhow::bail!(
            "activity rail row {activity_row} and jump control row {jump_row} diverged:\n{}",
            harness.screen().contents()
        );
    }
    Ok(())
}

fn assert_help_overlay_dismissed(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("Keyboard shortcuts") {
        anyhow::bail!("help overlay still visible after Esc:\n{screen}");
    }
    if !screen.contains("gpt-5.5") {
        anyhow::bail!("session chrome missing after dismissing help:\n{screen}");
    }
    Ok(())
}

fn assert_slash_palette_filtered_to_model(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("/model") {
        anyhow::bail!("filtered slash palette missing /model:\n{screen}");
    }
    // After `/mod`, earlier alphabetical commands must leave the short list.
    for leftover in ["/agents", "/changelog", "/compact", "/config", "/diff"] {
        if screen.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with(&format!("> {leftover}"))
                || trimmed.starts_with(&format!("  {leftover}"))
                || trimmed.starts_with(&format!("{leftover} "))
        }) {
            anyhow::bail!("slash palette still listed {leftover} after /mod filter:\n{screen}");
        }
    }
    Ok(())
}

fn assert_file_palette_filtered_to_alpha(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains(FILE_ALPHA) {
        anyhow::bail!("file palette missing {FILE_ALPHA}:\n{screen}");
    }
    if screen.contains(FILE_BETA) {
        anyhow::bail!("file palette still listed {FILE_BETA} after @alpha filter:\n{screen}");
    }
    Ok(())
}

fn assert_file_path_inserted(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let inserted = format!("@{FILE_ALPHA}");
    if !screen.contains(&inserted) {
        anyhow::bail!("composer missing inserted path {inserted}:\n{screen}");
    }
    Ok(())
}
