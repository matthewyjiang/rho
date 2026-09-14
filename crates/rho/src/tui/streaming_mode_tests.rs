use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;

use super::{ParagraphBoundary, StreamingMode};
use crate::tui::{StreamKind, StreamUi};

// Covers: split delimiters and Unicode must not drop or prematurely release text.
// Owner: incremental paragraph release policy.
#[test]
fn paragraph_boundaries_across_deltas() {
    for (chunks, expected) in [
        (vec!["hé", "llo\n", "\n尾"], vec!["", "", "héllo\n\n"]),
        (vec!["a\r\n ", "\t\r", "\nb"], vec!["", "", "a\r\n \t\r\n"]),
        (vec!["a\nb", "\nc"], vec!["", ""]),
        (
            vec!["a\n\nb\n\nc", "\n", "\nd"],
            vec!["a\n\nb\n\n", "", "c\n\n"],
        ),
    ] {
        let mut boundary = ParagraphBoundary::default();
        let mut held = String::new();
        let actual: Vec<String> = chunks
            .iter()
            .map(|chunk| {
                held.push_str(chunk);
                let end = boundary.release_end(&held);
                held.drain(..end).collect()
            })
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(format!("{}{held}", actual.concat()), chunks.concat());
    }
}

// Covers: buffered modes must not start pacing ticks or lose the final partial
// paragraph at message boundaries. Uses a manual clock, not wall-clock timing.
#[test]
fn buffered_release_and_boundary_flush() {
    let now = Instant::now();
    for (mode, expected) in [
        (StreamingMode::Paragraph, "first\n\n"),
        (StreamingMode::Off, ""),
    ] {
        let mut streams = StreamUi {
            mode,
            ..Default::default()
        };
        streams.current_stream_kind = Some(StreamKind::Assistant);
        streams.push_delta(StreamKind::Assistant, "first\n\nunfinished", now);
        assert_eq!(streams.assistant_stream.pending_text(), expected);
        assert_eq!(streams.stream_tick_deadline, None);
        assert!(!streams.on_tick(now + Duration::from_secs(1)));
        streams.flush_hold(StreamKind::Assistant);
        assert_eq!(
            streams.assistant_stream.pending_text(),
            "first\n\nunfinished"
        );
        streams.reset();
        streams.push_delta(StreamKind::Assistant, "next\n\n", now);
        assert_eq!(
            streams.assistant_stream.pending_text(),
            if mode == StreamingMode::Paragraph {
                "next\n\n"
            } else {
                ""
            }
        );
    }
}
