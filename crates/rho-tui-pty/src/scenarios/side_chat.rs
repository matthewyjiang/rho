//! `/side` and `/btw` overlay scenarios.

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{type_during_stream::wait_for_later_flood_event, SETTLE, STARTUP, STREAM};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

// Covers: /side opens the aside overlay and Esc returns to the session
// without writing the aside into the parent transcript.
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
    Step::Phase("dismiss"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::Custom(assert_side_overlay_dismissed),
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

// Covers: /btw is the /side alias, including inline send.
// Owner: interactive TUI
const SIDE_BTW_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_btw"),
    Step::SubmitText("/btw hello from aside"),
    Step::WaitText {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "hello from aside",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "fixture response: hello from aside",
        timeout: STARTUP,
    },
    Step::Custom(assert_side_prompt_stayed_out_of_parent),
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
);

// Covers: opening /side during a parent turn must not abort that turn when
// Esc closes the overlay.
// Owner: interactive TUI
const SIDE_DURING_TURN_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_flood"),
    Step::SubmitText("fixture input flood"),
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
    Step::Phase("overlay_esc_does_not_abort"),
    Step::Key(Key::Esc),
    Step::WaitTextGone {
        text: "Side chat",
        timeout: SETTLE,
    },
    Step::Custom(wait_for_later_flood_event),
    Step::WaitTextGone {
        text: "model interrupted",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const SIDE_DURING_TURN_SCENARIO: Scenario = Scenario::new(
    "side_during_turn",
    "Open side chat during a parent turn without aborting it",
    SIZE,
    SIDE_DURING_TURN_STEPS,
    /* smoke */ false,
);

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

fn assert_side_prompt_stayed_out_of_parent(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("Side chat") {
        anyhow::bail!("side overlay closed before the aside finished:\n{screen}");
    }
    if !screen.contains("hello from aside") {
        anyhow::bail!("aside prompt missing from overlay:\n{screen}");
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
