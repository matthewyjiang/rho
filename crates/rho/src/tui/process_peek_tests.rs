use pretty_assertions::assert_eq;

use super::{peek_body_model, PeekBodyLine, ProcessPeekView};
use crate::tools::process::{Chunk, HostProcessView, LiveProcessSummary, State, Stream};

fn chunk(stream: Stream, text: &str) -> Chunk {
    Chunk {
        cursor: 0,
        stream,
        text: text.to_owned(),
    }
}

// Covers: peek body must tag stderr and surface eviction before remaining output.
// Owner: pure unit (peek line model)
#[test]
fn peek_body_model_marks_eviction_and_stream() {
    let cases = [
        (Vec::new(), false, Vec::new()),
        (
            vec![chunk(Stream::Stdout, "hello\n")],
            true,
            vec![
                PeekBodyLine::Evicted,
                PeekBodyLine::Output {
                    stream: Stream::Stdout,
                    text: "hello".into(),
                },
            ],
        ),
        (
            vec![
                chunk(Stream::Stdout, "out\n"),
                chunk(Stream::Stderr, "err\n"),
            ],
            false,
            vec![
                PeekBodyLine::Output {
                    stream: Stream::Stdout,
                    text: "out".into(),
                },
                PeekBodyLine::Output {
                    stream: Stream::Stderr,
                    text: "err".into(),
                },
            ],
        ),
        (
            vec![chunk(Stream::Stdout, "hel"), chunk(Stream::Stdout, "lo\n")],
            false,
            vec![PeekBodyLine::Output {
                stream: Stream::Stdout,
                text: "hello".into(),
            }],
        ),
    ];
    for (chunks, truncated, expected) in cases {
        assert_eq!(peek_body_model(&chunks, truncated), expected);
    }
}

fn daemon_view() -> ProcessPeekView {
    ProcessPeekView::of_daemon(&LiveProcessSummary {
        process_id: "p1".into(),
        command: "tail -f log".into(),
        state: State::Running,
        elapsed_seconds: 1,
        quiet_seconds: None,
        exit_code: None,
    })
}

fn daemon_answer(first_cursor: u64, next_cursor: u64, cursors: &[u64]) -> HostProcessView {
    let mut view = daemon_view().view;
    view.snapshot.first_cursor = first_cursor;
    view.snapshot.next_cursor = next_cursor;
    view.snapshot.chunks = cursors
        .iter()
        .map(|&cursor| Chunk {
            cursor,
            ..chunk(Stream::Stdout, "line\n")
        })
        .collect();
    view
}

// Covers: a daemon peek view reads output in pieces, so each answer must add
// to the chunks held, drop the ones the daemon evicted since, say output
// was evicted, and move the cursor the next read starts from.
// Owner: pure unit (peek view of a daemon session's process)
#[test]
fn daemon_answers_extend_the_output_held() {
    let mut view = daemon_view();
    let held = |view: &ProcessPeekView| {
        let snapshot = &view.view.snapshot;
        (
            snapshot
                .chunks
                .iter()
                .map(|chunk| chunk.cursor)
                .collect::<Vec<_>>(),
            snapshot.truncated,
            view.daemon_cursor().1,
        )
    };

    view.extend(daemon_answer(0, 3, &[0, 1, 2]));
    assert_eq!(held(&view), (vec![0, 1, 2], false, 3));

    // Nothing new: the chunks held stay.
    view.extend(daemon_answer(0, 3, &[]));
    assert_eq!(held(&view), (vec![0, 1, 2], false, 3));

    // The daemon evicted 0 and 1 and read two more.
    view.extend(daemon_answer(2, 5, &[3, 4]));
    assert_eq!(held(&view), (vec![2, 3, 4], true, 5));
}
