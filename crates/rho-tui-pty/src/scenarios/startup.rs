//! First-frame paint and the core startup-stream-exit smoke path.

use std::time::Duration;

use crate::{
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

const SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

pub(super) const STARTUP_FIRST_FRAME_SCENARIO: Scenario = Scenario::new(
    "startup_first_frame",
    "Measure time to first session chrome, then exit",
    SIZE,
    STARTUP_FIRST_FRAME_STEPS,
    false,
);

pub(super) const STARTUP_STREAM_EXIT_SCENARIO: Scenario = Scenario::new(
    "startup_stream_exit",
    "Start, stream a fixture response, and exit cleanly",
    SIZE,
    STARTUP_STREAM_EXIT_STEPS,
    true,
);

pub(super) const STARTUP_PROMPT_STREAM_EXIT_SCENARIO: Scenario = Scenario::new(
    "startup_prompt_stream_exit",
    "Launch with --prompt and stream a fixture response without typing",
    SIZE,
    STARTUP_PROMPT_STREAM_EXIT_STEPS,
    true,
)
.with_args(&["--prompt", "fixture stream"]);

const STARTUP_FIRST_FRAME_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "rho",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::ExitCommand,
];

const STARTUP_STREAM_EXIT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "rho",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("submit_stream"),
    Step::SubmitText("fixture stream"),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "part two",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const STARTUP_PROMPT_STREAM_EXIT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "rho",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("startup_prompt_stream"),
    Step::WaitText {
        text: "fixture stream",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "part two",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];
