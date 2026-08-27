//! Steering acceptance, apply-into-transcript, and retract flows.

use anyhow::Result;

use crate::{
    harness::PtyHarness,
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP, STREAM};

fn assert_applied_steer_is_user_line(harness: &mut PtyHarness) -> Result<()> {
    const STEER: &str = "fixture steer detail";
    let has_standalone_line = harness
        .screen()
        .rows_text()
        .iter()
        .any(|row| row.trim() == STEER);
    if !has_standalone_line {
        anyhow::bail!(
            "applied steer did not appear as a transcript user line:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
}

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

pub(super) const QUEUE_FOLLOW_UP_DURING_TURN_SCENARIO: Scenario = Scenario::new(
    "queue_follow_up_during_turn",
    "Alt+Enter and Ctrl+Enter queue follow-ups during a turn, not steers",
    DEFAULT_SIZE,
    QUEUE_FOLLOW_UP_DURING_TURN_STEPS,
    false,
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

const QUEUE_FOLLOW_UP_DURING_TURN_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_turn"),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::Phase("queue_alt_enter"),
    Step::TypeText("first follow-up"),
    Step::Key(Key::AltEnter),
    Step::WaitText {
        text: "1 follow-up",
        timeout: STREAM,
    },
    Step::Phase("queue_ctrl_enter"),
    Step::TypeText("second follow-up"),
    Step::Key(Key::CtrlEnter),
    Step::WaitText {
        text: "2 follow-ups",
        timeout: STREAM,
    },
    Step::WaitTextGone {
        text: "STEER",
        timeout: STREAM,
    },
    Step::Phase("abort"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::CtrlCExit,
];
