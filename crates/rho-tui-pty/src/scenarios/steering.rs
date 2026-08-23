//! Steering acceptance, apply-into-transcript, and retract flows.

use crate::{
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{assert_helpers::assert_applied_steer_is_user_line, DEFAULT_SIZE, STARTUP, STREAM};

pub(super) const STEER_APPEARS_IN_TRANSCRIPT_SCENARIO: Scenario = Scenario::new(
    "steer_appears_in_transcript",
    "Applied steering appears as a transcript user message",
    DEFAULT_SIZE,
    STEER_APPEARS_IN_TRANSCRIPT_STEPS,
    true,
);

pub(super) const RETRACT_STEERING_DURING_TOOL_SCENARIO: Scenario = Scenario::new(
    "retract_steering_during_tool",
    "Inspect and retract steering while a tool is running",
    DEFAULT_SIZE,
    RETRACT_STEERING_DURING_TOOL_STEPS,
    true,
);

const STEER_APPEARS_IN_TRANSCRIPT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_turn"),
    Step::SubmitText("fixture steering"),
    Step::WaitText {
        text: "initial turn waiting for steering",
        timeout: STREAM,
    },
    Step::Phase("steer"),
    Step::SubmitText("fixture steer detail"),
    Step::WaitText {
        text: "pending input",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "STEER",
        timeout: STREAM,
    },
    Step::Phase("applied"),
    Step::WaitText {
        text: "steering applied exactly once: fixture steer detail",
        timeout: STREAM,
    },
    Step::WaitTextGone {
        text: "STEER",
        timeout: STREAM,
    },
    Step::Custom(assert_applied_steer_is_user_line),
    Step::ExitCommand,
];

const RETRACT_STEERING_DURING_TOOL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_tool"),
    Step::SubmitText("fixture progress tool"),
    Step::WaitText {
        text: "deterministic progress update one",
        timeout: STREAM,
    },
    Step::Phase("steer"),
    Step::SubmitText("keep the public API unchanged"),
    Step::WaitText {
        text: "pending input",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "STEER",
        timeout: STREAM,
    },
    Step::Phase("retract"),
    Step::Key(Key::AltUp),
    Step::WaitText {
        text: "editing retracted steer",
        timeout: STREAM,
    },
    Step::Key(Key::Ctrl('c')),
    Step::WaitText {
        text: "input cleared",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "progress tool lifecycle complete",
        timeout: STREAM,
    },
    Step::ExitCommand,
];
