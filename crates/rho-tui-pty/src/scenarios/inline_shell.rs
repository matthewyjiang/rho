//! Inline shell streaming and cancellation while a model turn is active.

use std::time::Duration;

use super::{
    assert_helpers::{assert_idle_shell_still_streaming, assert_inline_shell_cancelled},
    DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    keys::Key,
    scenario::{Scenario, Step},
};

pub(super) const INLINE_SHELL_DURING_TURN_SCENARIO: Scenario = Scenario::new(
    "inline_shell_during_turn",
    "Run local and context shell commands during an active turn",
    DEFAULT_SIZE,
    INLINE_SHELL_DURING_TURN_STEPS,
    /*smoke*/ false,
);

const INLINE_SHELL_DURING_TURN_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("!!printf idle-stream-%s start; sleep 2; printf idle-stream-%s end"),
    Step::WaitText {
        text: "idle-stream-start",
        timeout: STREAM,
    },
    Step::Custom(assert_idle_shell_still_streaming),
    Step::WaitText {
        text: "idle-stream-end",
        timeout: STREAM,
    },
    Step::SubmitText("!!printf cancel-%s started; sleep 1; printf cancel-%s escaped-output"),
    Step::WaitText {
        text: "cancel-started",
        timeout: STREAM,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "cancelled",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(1_200),
        timeout: STREAM,
    },
    Step::Custom(assert_inline_shell_cancelled),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::SubmitText("!!printf streamed-%s start; sleep 1; printf streamed-%s end"),
    Step::WaitText {
        text: "streamed-start",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "streamed-end",
        timeout: STREAM,
    },
    Step::SubmitText("!printf context-%s during-turn"),
    Step::WaitText {
        text: "context-during-turn",
        timeout: STREAM,
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];
