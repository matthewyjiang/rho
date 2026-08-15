//! `/attach` overlay for running subagents.

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
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: std::time::Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const ATTACH_PICKER_SCENARIO: Scenario = Scenario::new(
    "attach_picker",
    "Open /attach and list a running subagent by role",
    DEFAULT_SIZE,
    ATTACH_PICKER_STEPS,
    /*smoke*/ false,
);
