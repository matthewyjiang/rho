use std::time::Duration;

use crate::harness::WaitTimeout;
use crate::{keys::Key, scenario::Step};

const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

const QUIET: Step = Step::WaitQuiet {
    quiet_for: Duration::from_millis(150),
    timeout: SETTLE,
};

/// Create a saved session, open the `/sessions` hub, browse into its
/// directory, resume the session, then exercise the directory-wide delete:
/// cancel first, then confirm and verify the current session survives.
pub(super) const SESSIONS_HUB_STEPS: &[Step] = &[
    Step::Phase("create_saved_session"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("sessions hub target"),
    Step::WaitText {
        text: "fixture response: sessions hub target",
        timeout: STREAM,
    },
    Step::Phase("start_fresh_session"),
    Step::Key(Key::Ctrl('r')),
    Step::WaitText {
        text: "conversation reset",
        timeout: SETTLE,
    },
    Step::Phase("open_sessions_hub"),
    Step::SubmitText("/sessions"),
    Step::WaitText {
        text: "All sessions",
        timeout: SETTLE,
    },
    // Rows label sessions by stored title, so confirm the saved session by
    // selecting it and reading its last user message in the detail pane.
    Step::Key(Key::Down),
    Step::WaitText {
        text: "last: sessions hub target",
        timeout: SETTLE,
    },
    Step::Key(Key::Up),
    Step::Phase("browse_directory"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Esc back",
        timeout: SETTLE,
    },
    Step::Phase("resume_from_directory"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "resumed session",
        timeout: SETTLE,
    },
    Step::Phase("reopen_hub"),
    Step::SubmitText("/sessions"),
    Step::WaitText {
        text: "All sessions",
        timeout: SETTLE,
    },
    Step::Phase("cancel_directory_delete"),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "Delete all sessions in",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "All sessions",
        timeout: SETTLE,
    },
    QUIET,
    Step::Phase("confirm_directory_delete_keeps_current"),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "Delete all sessions in",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "kept the current session",
        timeout: SETTLE,
    },
    Step::Phase("current_session_delete_is_refused"),
    Step::Key(Key::Down),
    Step::Key(Key::Char('d')),
    Step::WaitText {
        text: "cannot delete the current session",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    QUIET,
    Step::ExitCommand,
];
