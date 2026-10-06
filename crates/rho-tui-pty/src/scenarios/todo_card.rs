//! Checklist status and overflow through a native tool call, even in Plan mode.

use crate::{
    keys::Key,
    scenario::{Scenario, Step},
};

use super::{DEFAULT_SIZE, STARTUP, STREAM};

pub(super) const TODO_CARD_SCENARIO: Scenario = Scenario::new(
    "todo_card",
    "Render native checklist statuses and overflow in Plan mode",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture todo"),
        Step::WaitText {
            text: "todo checklist complete",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "Worked for",
            timeout: STREAM,
        },
        // The default card budget is ten terminal rows; expansion reveals the
        // eleventh fact, which reports the two items outside the list preview.
        Step::Key(Key::Ctrl('o')),
        Step::WaitText {
            text: "2 more",
            timeout: STREAM,
        },
        Step::AssertText("◐ implement checklist"),
        Step::AssertText("☑ inspect requirements"),
        Step::AssertText("☐ remaining step 4"),
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_args(&["--permission-mode", "plan"]);
