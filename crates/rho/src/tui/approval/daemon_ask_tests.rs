use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{ApprovalAsk, ApprovalSource, Operation, SessionControlAction, SessionId};

use super::super::render::ApprovalPrompt;

// Covers: a daemon session's session-control approval names the action, the
// session it acts on (none for one it starts), the text it would send, and
// the session's workspace, with control characters from the request made
// harmless (#1714).
// Owner: TUI approval prompt for daemon asks.
#[test]
fn a_session_control_ask_names_its_target() {
    let ask = |action, session: Option<&str>, text: Option<&str>| ApprovalAsk {
        source: ApprovalSource::BuiltInTool {
            name: "sessions".into(),
        },
        operation: Operation::ControlSession {
            action,
            session: session.map(SessionId::new),
            workspace: PathBuf::from("/work/other\u{1b}[2J"),
            text: text.map(str::to_owned),
        },
        reason: String::new(),
    };
    let shown = |ask: ApprovalAsk| {
        let prompt = ApprovalPrompt::of_daemon_ask(&ask);
        (prompt.title, prompt.details.primary, prompt.details.meta)
    };
    let workspace = || vec![r"workspace /work/other\u{1b}[2J".to_owned()];

    assert_eq!(
        [
            shown(ask(SessionControlAction::New, None, Some("run the tests"))),
            shown(ask(SessionControlAction::Cancel, Some("s-1"), None)),
        ],
        [
            (
                "sessions wants to control another session".to_owned(),
                vec!["new session".to_owned(), "run the tests".to_owned()],
                workspace(),
            ),
            (
                "sessions wants to control another session".to_owned(),
                vec!["cancel session s-1".to_owned()],
                workspace(),
            ),
        ]
    );
}
