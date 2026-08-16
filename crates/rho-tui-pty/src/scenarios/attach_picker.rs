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

pub(super) const ATTACH_PICKER_EMPTY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("open_empty_attach_picker"),
    Step::SubmitText("/attach"),
    Step::WaitText {
        text: "attach subagent",
        timeout: SETTLE,
    },
    Step::AssertText("SUBAGENTS"),
    Step::AssertText("0/0"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: std::time::Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) const ATTACH_PICKER_EMPTY_SCENARIO: Scenario = Scenario::new(
    "attach_picker_empty",
    "Open /attach with no running subagents and keep the empty overlay",
    DEFAULT_SIZE,
    ATTACH_PICKER_EMPTY_STEPS,
    /*smoke*/ false,
);

pub(super) const ATTACH_CLI_EMPTY_STEPS: &[Step] = &[
    Step::Phase("open_empty_cli_attach_picker"),
    Step::WaitText {
        text: "attach subagent",
        timeout: STARTUP,
    },
    Step::AssertText("SUBAGENTS"),
    Step::AssertText("0/0"),
    Step::Key(Key::Esc),
    Step::WaitExit { timeout: SETTLE },
];

pub(super) const ATTACH_CLI_EMPTY_SCENARIO: Scenario = Scenario::new(
    "attach_cli_empty",
    "Open rho attach with no running subagents and keep the empty overlay",
    DEFAULT_SIZE,
    ATTACH_CLI_EMPTY_STEPS,
    /*smoke*/ false,
)
.with_args(&["attach"]);
