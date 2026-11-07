//! `/attach` overlay for workspace subagents.

use super::{DEFAULT_SIZE, SETTLE, STARTUP, STREAM};
use crate::{
    keys::Key,
    scenario::{Scenario, Step},
};

pub(super) const ATTACH_PICKER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_long_running_subagent"),
    Step::SubmitText("fixture subagent rail"),
    Step::WaitText {
        text: "subagent rail fixture dispatched",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "worker",
        timeout: SETTLE,
    },
    Step::Phase("open_attach_picker"),
    Step::SubmitText("/attach"),
    Step::WaitText {
        text: "attach subagent",
        timeout: SETTLE,
    },
    Step::AssertText("SUBAGENTS"),
    Step::AssertText("worker"),
    // The run card shows what the run was asked, read from its journal.
    Step::Phase("run_card"),
    Step::WaitText {
        text: "fixture delay",
        timeout: SETTLE,
    },
    Step::AssertText("TASK"),
    Step::AssertText("LATEST"),
    Step::AssertText("Elapsed"),
    Step::Key(Key::Esc),
    // The running worker keeps the activity spinner animating, so the screen
    // never goes quiet here; wait for the picker itself to close instead.
    Step::WaitTextGone {
        text: "attach subagent",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const ATTACH_PICKER_SCENARIO: Scenario = Scenario::new(
    "attach_picker",
    "Open /attach, list a running subagent by role, and show its run card",
    DEFAULT_SIZE,
    ATTACH_PICKER_STEPS,
    /*smoke*/ false,
);
