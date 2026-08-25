use pretty_assertions::assert_eq;

use super::{peek_body_model, PeekBodyLine};
use crate::tools::process::{Chunk, Stream};

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
    ];
    for (chunks, truncated, expected) in cases {
        assert_eq!(peek_body_model(&chunks, truncated), expected);
    }
}
