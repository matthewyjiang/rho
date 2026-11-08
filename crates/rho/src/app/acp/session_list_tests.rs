use std::path::Path;

use agent_client_protocol::schema::v1::{SessionId, SessionInfo};
use pretty_assertions::assert_eq;

use super::session_info;
use crate::session::SessionSummary;

fn summary(id: &str, cwd: &Path, updated_at: u64) -> SessionSummary {
    SessionSummary {
        id: id.to_owned(),
        path: cwd.join(format!("{id}.jsonl")),
        cwd: cwd.to_path_buf(),
        created_at: 0,
        updated_at,
        message_count: 2,
        title: None,
        first_user_message: None,
        last_user_message: None,
        added_dirs: Vec::new(),
        origin: None,
    }
}

// Covers: listed entries carry the label /resume shows (the title, else the
// first user message on one line) and an RFC 3339 updatedAt, the format Zed
// parses; an unparseable date would drop the entry's recency.
// Owner: ACP session/list mapping
#[test]
fn entries_carry_resume_label_and_rfc3339_time() {
    let workspace = tempfile::tempdir().unwrap();
    let cwd = workspace.path();
    let long = "word ".repeat(40);
    let cases = [
        (
            Some("Fix login"),
            Some("ignored"),
            Some("Fix login".to_owned()),
        ),
        (
            None,
            Some("  fix the\n  login bug \n"),
            Some("fix the login bug".to_owned()),
        ),
        (None, Some(long.as_str()), Some(format!("{}…", &long[..79]))),
        (None, Some(" \n "), None),
        (None, None, None),
    ];
    for (title, first_user_message, expected) in cases {
        let mut stored = summary("one", cwd, 1_792_133_984);
        stored.title = title.map(str::to_owned);
        stored.first_user_message = first_user_message.map(str::to_owned);

        assert_eq!(
            session_info(stored),
            SessionInfo::new(SessionId::new("one"), cwd)
                .title(expected)
                .updated_at("2026-10-16T06:59:44Z".to_owned())
        );
    }
}
