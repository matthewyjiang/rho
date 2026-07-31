use std::time::Duration;

use crate::{
    harness::WaitTimeout,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP};

const RESPONSE: WaitTimeout = WaitTimeout::secs(20, "paste response");

const PASTE_MULTILINE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("delete_collapsed_paste"),
    Step::Paste("discard one\ndiscard two\ndiscard three"),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Key(Key::Backspace),
    Step::SubmitText("fixture stream"),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: RESPONSE,
    },
    Step::WaitText {
        text: "part two",
        timeout: RESPONSE,
    },
    Step::Phase("submit_multiline_paste"),
    Step::Paste("line one\n/not-a-command\nline three"),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "fixture response:",
        timeout: RESPONSE,
    },
    Step::ExitCommand,
];

pub(super) const PASTE_MULTILINE_SCENARIO: Scenario = Scenario::new(
    "paste_multiline",
    "Paste multiline text without treating embedded lines as commands",
    PtySize {
        rows: 28,
        cols: 100,
    },
    PASTE_MULTILINE_STEPS,
    false,
);
