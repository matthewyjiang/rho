use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: a listed session travels with its absent optionals left out, so
// ACP's session/list and the TUI's hub read one shape (#1670, #1667).
// Owner: protocol wire contract.
#[test]
fn a_session_list_page_round_trips_through_its_wire_form() {
    let page = SessionListResponse {
        sessions: vec![
            SessionSummary {
                session: SessionId::new("s2"),
                cwd: "/w/app".into(),
                added_dirs: vec!["/w/lib".into()],
                title: Some("Fix login".into()),
                first_user_message: Some("fix the login bug".into()),
                last_user_message: Some("now add a test".into()),
                updated_at: 1_792_133_984,
            },
            SessionSummary {
                session: SessionId::new("s1"),
                cwd: "/w/app".into(),
                added_dirs: Vec::new(),
                title: None,
                first_user_message: None,
                last_user_message: None,
                updated_at: 1_792_000_000,
            },
        ],
        next_cursor: Some("opaque".into()),
    };
    let wire = json!({
        "sessions": [
            {
                "session": "s2",
                "cwd": "/w/app",
                "added_dirs": ["/w/lib"],
                "title": "Fix login",
                "first_user_message": "fix the login bug",
                "last_user_message": "now add a test",
                "updated_at": 1_792_133_984,
            },
            {"session": "s1", "cwd": "/w/app", "added_dirs": [], "updated_at": 1_792_000_000},
        ],
        "next_cursor": "opaque",
    });
    assert_eq!(serde_json::to_value(&page).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<SessionListResponse>(wire).unwrap(),
        page
    );
}
