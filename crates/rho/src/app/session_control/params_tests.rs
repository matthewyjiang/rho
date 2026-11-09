use pretty_assertions::assert_eq;

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
