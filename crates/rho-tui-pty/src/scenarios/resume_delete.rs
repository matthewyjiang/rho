use crate::harness::WaitTimeout;
use crate::{keys::Key, scenario::Step};

const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

/// Create a saved session, open the resume picker from a fresh session, cancel
/// delete, then confirm delete.
pub(super) const RESUME_PICKER_DELETE_STEPS: &[Step] = &[
    Step::Phase("create_saved_session"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("resume delete target"),
    Step::WaitText {
        text: "fixture response: resume delete target",
        timeout: STREAM,
    },
    Step::Phase("start_fresh_session"),
    Step::Key(Key::Ctrl('r')),
    Step::WaitText {
        text: "conversation reset",
        timeout: SETTLE,
    },
    Step::Phase("open_resume_picker"),
    Step::SubmitText("/resume"),
    Step::WaitText {
        text: "Resume session",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "resume delete target",
        timeout: SETTLE,
    },
    Step::Phase("cancel_delete"),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "Delete session",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Resume session",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "resume delete target",
        timeout: SETTLE,
    },
    Step::Phase("confirm_delete"),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "Delete session",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "deleted session",
        timeout: SETTLE,
    },
    // The delete notice toast overwrites the picker's own empty-list status,
    // so reopen the picker to observe the empty state.
    Step::Phase("empty_picker_after_delete"),
    Step::SubmitText("/resume"),
    Step::WaitText {
        text: "no saved sessions for this workspace",
        timeout: SETTLE,
    },
    Step::ExitCommand,
];
