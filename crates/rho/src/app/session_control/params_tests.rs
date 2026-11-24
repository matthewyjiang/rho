use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::*;

// Covers: the cursor `wait` hands out parses back to the same position, a
// stream id holding a colon included, and text it never handed out is
// refused rather than read as some position.
// Owner: session-control wait cursor.
#[test]
fn cursors_round_trip_and_garbage_is_refused() {
    let cursor = Cursor {
        stream: StreamId::new("a:b"),
        seq: Seq(42),
    };
    let text = format_cursor(&cursor);

    assert_eq!(parse_cursor(&text).unwrap(), cursor);
    for garbage in ["", "42", ":42", "stream:", "stream:x"] {
        assert!(parse_cursor(garbage).is_err(), "{garbage:?}");
    }
}

// Covers: `read` windows count characters, not bytes, report where the next
// page starts until the last, and clamp a start past the end.
// Owner: session-control read paging.
#[test]
fn read_windows_page_by_characters() {
    let session = SessionId::new("s");
    let text = "héllo wörld";
    let window = |start, chars| Window { start, chars };

    assert_eq!(
        [
            ReadResult::window(session.clone(), text, window(0, 5)),
            ReadResult::window(session.clone(), text, window(5, 100)),
            ReadResult::window(session.clone(), text, window(99, 5)),
        ],
        [
            ReadResult {
                session: session.clone(),
                text: "héllo".into(),
                start: 0,
                end: 5,
                total_chars: 11,
                next_start: Some(5),
            },
            ReadResult {
                session: session.clone(),
                text: " wörld".into(),
                start: 5,
                end: 11,
                total_chars: 11,
                next_start: None,
            },
            ReadResult {
                session,
                text: String::new(),
                start: 11,
                end: 11,
                total_chars: 11,
                next_start: None,
            },
        ]
    );
}

// Covers: an argument past its limit is refused naming the limit and the
// value asked for.
// Owner: session-control params.
#[test]
fn oversized_arguments_name_their_limit() {
    let wait = WaitParams {
        session: SessionId::new("s"),
        timeout_secs: Some(51),
        cursor: None,
    };
    let list = ListParams {
        limit: NonZeroUsize::new(1001),
        ..ListParams::default()
    };

    assert_eq!(
        [
            wait.timeout().unwrap_err().to_string(),
            list.limit().unwrap_err().to_string(),
        ],
        [
            "wait timeout: limit 50 s, asked 51 s; call again with the cursor to wait longer"
                .to_owned(),
            "list limit: limit 1000, asked 1001".to_owned(),
        ]
    );
}

/// A session job in `state`, opened by `opener`, with `title` while the
/// daemon has its session loaded.
fn session_job(state: JobState, opener: Opener, title: Option<&str>) -> JobSummary {
    let limit = match opener {
        Opener::Agent => json!({"depth": 1, "access": "write", "workspaces": "any"}),
        Opener::Person => Value::Null,
    };
    serde_json::from_value(json!({
        "id": "job",
        "kind": "session",
        "state": state,
        "workspace": "/work",
        "session": "s",
        "created_at": 1,
        "updated_at": 2,
        "grant_limit": limit,
        "live": title.map(|title| json!({
            "title": title,
            "model": {"provider": "fixture", "model": "fixture"},
        })),
    }))
    .unwrap()
}

// Covers: `list` keeps a session only if it passes every filter the call
// names: its state, who opened it, and, with `hide_idle`, having had a turn.
// Owner: session-control list filters.
#[test]
fn list_filters_keep_only_the_sessions_they_name() {
    let by = |opener| ListParams {
        opened_by: Some(opener),
        ..ListParams::default()
    };
    let started = || ListParams {
        hide_idle: true,
        ..ListParams::default()
    };
    let running = ListParams {
        state: Some(JobState::Running),
        ..ListParams::default()
    };
    let cases = [
        (ListParams::default(), JobState::Idle, Opener::Person, true),
        (by(Opener::Person), JobState::Running, Opener::Person, true),
        (by(Opener::Person), JobState::Running, Opener::Agent, false),
        (by(Opener::Agent), JobState::Running, Opener::Agent, true),
        (by(Opener::Agent), JobState::Running, Opener::Person, false),
        (started(), JobState::Idle, Opener::Person, false),
        (started(), JobState::Completed, Opener::Person, true),
        (running, JobState::Completed, Opener::Agent, false),
    ];

    let kept: Vec<bool> = cases
        .iter()
        .map(|(params, state, opener, _)| params.keeps(&session_job(*state, *opener, None)))
        .collect();

    assert_eq!(
        kept,
        cases.iter().map(|case| case.3).collect::<Vec<_>>(),
        "{cases:?}"
    );
}

// Covers: a `list` entry says who opened its session, from the limit the
// session opened under, and carries the title the daemon follows; a session
// without a title has no `title`.
// Owner: session-control list result.
#[test]
fn an_entry_says_who_opened_its_session_and_its_title() {
    let entry = |job| serde_json::to_value(SessionEntry::of_job(job, &HashMap::new())).unwrap();

    assert_eq!(
        [
            entry(session_job(
                JobState::Running,
                Opener::Person,
                Some("Fix the flaky test")
            )),
            entry(session_job(JobState::Completed, Opener::Agent, None)),
        ],
        [
            json!({
                "session": "s",
                "workspace": "/work",
                "state": "running",
                "opened_by": "person",
                "title": "Fix the flaky test",
                "updated_at": 2,
            }),
            json!({
                "session": "s",
                "workspace": "/work",
                "state": "completed",
                "opened_by": "agent",
                "updated_at": 2,
            }),
        ]
    );
}
