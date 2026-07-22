use crate::harness::WaitTimeout;
use crate::{keys::Key, scenario::Step};

const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

pub(super) const CONVERSATION_TREE_STEPS: &[Step] = &[
    Step::Phase("create_linear_history"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("tree first"),
    Step::WaitText {
        text: "fixture response: tree first",
        timeout: STREAM,
    },
    Step::SubmitText("tree second"),
    Step::WaitText {
        text: "fixture response: tree second",
        timeout: STREAM,
    },
    Step::Phase("restore_first_turn"),
    Step::SubmitText("/tree"),
    Step::WaitText {
        text: "Conversation tree",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "tree first",
        timeout: SETTLE,
    },
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "restored conversation state",
        timeout: STREAM,
    },
    Step::Phase("create_branch"),
    Step::SubmitText("tree branch"),
    Step::WaitText {
        text: "fixture response: tree branch",
        timeout: STREAM,
    },
    Step::SubmitText("/tree"),
    Step::WaitText {
        text: "tree second",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "tree branch",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
