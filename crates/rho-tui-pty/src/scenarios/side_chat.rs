//! `/side` and `/btw` overlay scenarios.

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

// Covers: /side opens the aside overlay and Esc returns to the session
// without writing the aside into the parent transcript. Idle Ctrl+C belongs
// to the aside, even when its composer is empty, and must not quit the parent.
// Owner: interactive TUI
const SIDE_OVERLAY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_side"),
    Step::SubmitText("/side"),
    Step::WaitText {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::Custom(assert_side_overlay_open),
    Step::Phase("type_j_inserts"),
    Step::TypeText("just"),
    Step::WaitText {
        text: "just",
        timeout: SETTLE,
    },
    Step::Phase("clear_aside_then_interrupt_empty_composer"),
    Step::Key(Key::Ctrl('c')),
    Step::WaitTextGone {
        text: "just",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::Key(Key::Ctrl('c')),
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::SubmitText("parent still accepts input"),
    Step::WaitText {
        text: "fixture response: parent still accepts input",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const SIDE_OVERLAY_SCENARIO: Scenario = Scenario::new(
    "side_overlay",
    "Open the side chat overlay and dismiss it with Esc",
    SIZE,
    SIDE_OVERLAY_STEPS,
    /* smoke */ false,
);

// Covers: empty /side while the overlay is open toggles it closed.
// Owner: interactive TUI
const SIDE_TOGGLE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_side"),
    Step::SubmitText("/side"),
    Step::WaitText {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::Phase("toggle_close"),
    Step::SubmitText("/side"),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::Custom(assert_side_overlay_dismissed),
    Step::ExitCommand,
];

pub(super) const SIDE_TOGGLE_SCENARIO: Scenario = Scenario::new(
    "side_toggle",
    "Toggle the side chat overlay closed with a second /side",
    SIZE,
    SIDE_TOGGLE_STEPS,
    /* smoke */ false,
);

// Covers: /btw replies retain transcript styling and code-block copy targets
// through a resize into a scrolled viewport, without entering the parent transcript.
// Owner: interactive TUI
const SIDE_BTW_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_btw"),
    Step::SubmitText("/btw **hello from aside** and `code`"),
    Step::WaitText {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "hello from aside",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "fixture response: hello from aside and code",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "Enter send   Esc close",
        timeout: SETTLE,
    },
    Step::Custom(assert_side_transcript_rendered),
    Step::Phase("copy_side_code"),
    Step::SubmitText("fixture code block"),
    Step::WaitText {
        text: "COPY",
        timeout: STARTUP,
    },
    Step::Custom(copy_visible_code),
    Step::Phase("resize_side_transcript"),
    Step::Resize { rows: 12, cols: 58 },
    Step::Custom(copy_visible_code),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::Custom(assert_side_answer_stayed_out_of_parent),
    Step::ExitCommand,
];

pub(super) const SIDE_BTW_SCENARIO: Scenario = Scenario::new(
    "side_btw",
    "Open side chat with /btw and send an inline prompt",
    SIZE,
    SIDE_BTW_STEPS,
    /* smoke */ false,
)
// Exercise OSC52 on every host instead of using the runner's native clipboard.
// Remote sessions cannot read the clipboard; right-click paste needs another fixture.
.with_env(&[("SSH_TTY", "rho-pty-clipboard")]);

// Covers: dismissing a running aside keeps both turns alive, restores parent
// input, and retains the background answer when reopened.
// Owner: interactive TUI
const SIDE_DURING_TURN_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_flood"),
    Step::SubmitText("fixture checkpointed input flood"),
    Step::WaitText {
        text: "input flood event 010",
        timeout: STREAM,
    },
    Step::Phase("open_side"),
    Step::SubmitText("/side"),
    Step::WaitText {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::SubmitText("fixture gated reply"),
    Step::WaitText {
        text: "reply waiting for release",
        timeout: STARTUP,
    },
    Step::Phase("overlay_esc_does_not_abort"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::TypeText("parent draft"),
    Step::WaitText {
        text: "parent draft",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::WaitTextGone {
        text: "parent draft",
        timeout: SETTLE,
    },
    Step::Custom(release_parent_and_side),
    Step::WaitText {
        text: "input flood event 200",
        timeout: STREAM,
    },
    Step::SubmitText("/side"),
    Step::WaitText {
        text: "reply completed after release",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Enter send   Esc close",
        timeout: SETTLE,
    },
    Step::Phase("cancel_only_the_aside"),
    // The first reply finished in the background; cancel a fresh gated turn.
    // The parent remains parked at event 200 until its own final Esc.
    Step::SubmitText("fixture gated reply"),
    Step::WaitText {
        text: "Esc close   Ctrl+C cancel",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::WaitText {
        text: "Enter send   Esc close",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::WaitTextGone {
        text: "model interrupted",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const SIDE_DURING_TURN_SCENARIO: Scenario = Scenario::new(
    "side_during_turn",
    "Background a running side chat and reopen its reply during a parent turn",
    SIZE,
    SIDE_DURING_TURN_STEPS,
    /* smoke */ false,
);

fn release_parent_and_side(harness: &mut PtyHarness) -> Result<()> {
    // Marker owners: rho-providers/src/providers/tui_fixture/stream_scenarios.rs.
    super::release_fixture(harness, ".rho-fixture-release-input-flood")?;
    super::release_fixture(harness, ".rho-fixture-release-reply")
}

fn assert_side_overlay_open(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("Side chat") {
        anyhow::bail!("side overlay title missing:\n{screen}");
    }
    if screen.contains("Search") {
        anyhow::bail!("side overlay used picker search chrome:\n{screen}");
    }
    Ok(())
}

fn copy_visible_code(harness: &mut PtyHarness) -> Result<()> {
    // A completed reply has its final spacer and stable copy-button geometry.
    harness.wait_for_text("Enter send   Esc close", SETTLE)?;
    harness.wait_for_text("COPY", SETTLE)?;
    let screen = harness.screen().contents();
    let (row, col) = screen
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find("COPY")
                .map(|col| (row as u16, line[..col].chars().count() as u16))
        })
        .ok_or_else(|| anyhow::anyhow!("missing code copy target:\n{screen}"))?;
    // Assert the clipboard wire payload, not a transient success notice.
    let sequence = b"\x1b]52;c;c2lkZV9jb3B5X3BheWxvYWQ=";
    let before = harness.raw_sequence_occurrences(sequence);
    // SGR mouse coordinates are 1-based.
    harness.mouse(crate::keys::MouseButton::Left, col + 1, row + 1, true)?;
    harness.mouse(crate::keys::MouseButton::Left, col + 1, row + 1, false)?;
    harness.wait_for_raw_sequence_occurrences(sequence, before + 1, SETTLE)
}

fn assert_side_overlay_dismissed(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("Side chat") {
        anyhow::bail!("side overlay still visible after close:\n{screen}");
    }
    if !screen.contains("gpt-5.5") {
        anyhow::bail!("session chrome missing after closing side chat:\n{screen}");
    }
    Ok(())
}

fn assert_side_transcript_rendered(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("Side chat") {
        anyhow::bail!("side overlay closed before the aside finished:\n{screen}");
    }
    if !screen.contains("hello from aside") {
        anyhow::bail!("aside prompt missing from overlay:\n{screen}");
    }
    let marker_cell = |marker: &str| {
        screen
            .lines()
            .enumerate()
            .find_map(|(row, line)| {
                let offset = line.find(marker)?;
                let col = line[..offset].chars().count();
                harness.screen().cell(row as u16, col as u16)
            })
            .ok_or_else(|| anyhow::anyhow!("missing styled marker {marker:?}:\n{screen}"))
    };
    let user = marker_cell("**hello from aside**")?;
    let prose = marker_cell("fixture response:")?;
    let emphasis = marker_cell("hello from aside and code")?;
    if user.bg == prose.bg || !emphasis.bold || prose.bold {
        anyhow::bail!(
            "side transcript lost user background or Markdown emphasis: \
             user={user:?}, prose={prose:?}, emphasis={emphasis:?}\n{screen}"
        );
    }
    Ok(())
}

fn assert_side_answer_stayed_out_of_parent(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("Side chat") {
        anyhow::bail!("side overlay still visible after close:\n{screen}");
    }
    if screen.contains("fixture response: hello from aside") {
        anyhow::bail!("aside answer leaked into the parent transcript:\n{screen}");
    }
    if !screen.contains("gpt-5.5") {
        anyhow::bail!("session chrome missing after closing side chat:\n{screen}");
    }
    Ok(())
}
