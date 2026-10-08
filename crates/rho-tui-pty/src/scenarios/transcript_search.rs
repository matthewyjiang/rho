//! Ctrl+F finds text in the rendered transcript, scrolls to each match, and
//! Esc returns to where the search started.

use std::time::Duration;

use crate::{
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{
    assert_helpers::wait_for_turn_completion_after, DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};

const LAST_LINE: &str = "fixture bulk one line 180";

// Covers: typing a query scrolls the viewport to the nearest earlier match,
// Down wraps from the newest match to the oldest, Up steps back, Esc restores
// the live bottom, and Enter keeps the found position.
// Owner: interactive TUI transcript search.
pub(super) const TRANSCRIPT_SEARCH_SCENARIO: Scenario = Scenario::new(
    "transcript_search",
    "Find transcript text with Ctrl+F, step between matches, and restore or keep the view",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("seed_transcript"),
        Step::SubmitText("fixture bulk one"),
        Step::Custom(|harness| wait_for_turn_completion_after(harness, LAST_LINE)),
        Step::Phase("incremental_find"),
        Step::Key(Key::Ctrl('f')),
        Step::WaitText {
            text: "Esc cancel",
            timeout: SETTLE,
        },
        Step::TypeText("LINE 007"),
        Step::WaitText {
            text: "fixture bulk one line 007",
            timeout: SETTLE,
        },
        Step::WaitTextGone {
            text: LAST_LINE,
            timeout: SETTLE,
        },
        Step::Phase("esc_restores_bottom"),
        Step::Key(Key::Esc),
        Step::WaitText {
            text: LAST_LINE,
            timeout: SETTLE,
        },
        Step::WaitTextGone {
            text: "fixture bulk one line 007",
            timeout: SETTLE,
        },
        Step::Phase("step_between_matches"),
        Step::Key(Key::Ctrl('f')),
        Step::TypeText("line 0"),
        Step::WaitText {
            text: "fixture bulk one line 099",
            timeout: SETTLE,
        },
        // Newest match is focused first; Down wraps to the oldest.
        Step::Key(Key::Down),
        Step::WaitText {
            text: "fixture bulk one line 001",
            timeout: SETTLE,
        },
        Step::Key(Key::Up),
        Step::WaitText {
            text: "fixture bulk one line 099",
            timeout: SETTLE,
        },
        Step::Phase("enter_keeps_position"),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "Type a message",
            timeout: SETTLE,
        },
        Step::AssertText("fixture bulk one line 099"),
        Step::WaitTextGone {
            text: LAST_LINE,
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
);

// Covers: a focused hit on the last screenful stays on screen while a turn
// appends more rows than the viewport holds, and Esc resumes following.
// Owner: interactive TUI transcript search.
pub(super) const TRANSCRIPT_SEARCH_HOLDS_HIT_SCENARIO: Scenario = Scenario::new(
    "transcript_search_holds_hit",
    "A focused transcript search hit is not scrolled away by appended rows",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Phase("start_turn"),
        Step::SubmitText("fixture slow stream"),
        Step::TypeText("fixture bulk one"),
        Step::Key(Key::AltEnter),
        Step::WaitText {
            text: "1 follow-up",
            timeout: STREAM,
        },
        Step::Phase("focus_bottom_hit"),
        Step::Key(Key::Ctrl('f')),
        Step::TypeText("slow stream"),
        Step::WaitText {
            text: "1/1",
            timeout: SETTLE,
        },
        Step::Phase("follow_up_appends"),
        Step::WaitTextGone {
            text: "1 follow-up",
            timeout: STREAM,
        },
        Step::WaitQuiet {
            quiet_for: Duration::from_millis(500),
            timeout: STREAM,
        },
        Step::AssertText("fixture slow stream"),
        Step::Phase("esc_follows_bottom"),
        Step::Key(Key::Esc),
        Step::Custom(|harness| wait_for_turn_completion_after(harness, LAST_LINE)),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
);
