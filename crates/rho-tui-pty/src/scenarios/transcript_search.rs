//! Ctrl+F finds text in the rendered transcript, scrolls to each match, and
//! Esc returns to where the search started.

use crate::{
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{assert_helpers::wait_for_turn_completion_after, DEFAULT_SIZE, SETTLE, STARTUP};

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
