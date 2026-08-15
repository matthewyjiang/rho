//! Live background-process rows in the activity rail.

use super::{DEFAULT_SIZE, STARTUP, STREAM};
use crate::scenario::{Scenario, Step};

pub(super) const PROCESS_RAIL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_process"),
    Step::SubmitText("fixture process rail"),
    Step::WaitText {
        text: "process rail fixture dispatched",
        timeout: STREAM,
    },
    Step::Phase("rail_survives_turn_end"),
    Step::WaitText {
        text: "└ sleep 60",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const PROCESS_RAIL_SCENARIO: Scenario = Scenario::new(
    "process_rail",
    "Show a live background process in the activity rail after the turn ends",
    DEFAULT_SIZE,
    PROCESS_RAIL_STEPS,
    /*smoke*/ false,
);
