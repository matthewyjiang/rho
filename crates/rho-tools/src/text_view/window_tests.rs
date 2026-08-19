use pretty_assertions::assert_eq;

use super::{format_window_bytes, read_text_window, ScanError, CHUNK_SIZE};
use crate::hashline::{format_hashline_view, FileHash};

fn numbered_log(lines: usize, pad: usize) -> String {
    let filler = "x".repeat(pad);
    let mut text = String::with_capacity(lines.saturating_mul(pad + 16));
    for index in 1..=lines {
        text.push_str(&format!("line-{index:06} {filler}\n"));
    }
    text
}

fn crossing_chunk_text() -> (String, usize) {
    let mut text = String::new();
    let mut line = 0usize;
    while text.len() + 5 < CHUNK_SIZE {
        line += 1;
        text.push_str("pad\n");
    }
    line += 1;
    text.push_str("CROSSING-LINE-XXXXXX\n");
    text.push_str("after\n");
    (text, line)
}

fn hashline_header<'a>(path: &'a str) -> impl FnOnce(Option<&str>) -> String + 'a {
    move |tag| match tag {
        Some(tag) => crate::hashline::format_header(path, tag),
        None => path.to_string(),
    }
}

// Covers: request-local scan must match the split view, including a line that
// starts before a 256 KiB chunk boundary
// Owner: text view window
#[test]
fn window_matches_split_across_chunk_windows() {
    let (crossing, crossing_line) = crossing_chunk_text();
    let log = numbered_log(8_000, 40);
    let no_nl = format!("{}tail", "x".repeat(CHUNK_SIZE + 10));
    assert!(
        log.len() > CHUNK_SIZE,
        "fixture must exceed one chunk, got {}",
        log.len()
    );

    let cases = [
        (crossing.as_str(), Some(1), Some(2)),
        (crossing.as_str(), Some(crossing_line), Some(2)),
        (crossing.as_str(), Some(crossing_line + 1), Some(1)),
        (log.as_str(), Some(1), Some(3)),
        (log.as_str(), Some(4_000), Some(5)),
        (log.as_str(), Some(7_990), Some(20)),
        (log.as_str(), None, Some(2)),
        (no_nl.as_str(), Some(1), Some(1)),
    ];
    for (text, offset, limit) in cases {
        let expected = format_hashline_view("log.txt", text, offset, limit).unwrap();
        let actual = format_window_bytes(
            text.as_bytes(),
            offset,
            limit,
            Some(FileHash::new()),
            hashline_header("log.txt"),
        )
        .unwrap();
        assert_eq!(actual, expected, "offset={offset:?} limit={limit:?}");
    }
}

// Covers: a line that continues past a chunk with no extra newline still hashes
// Owner: text view window
#[test]
fn window_reads_partial_line_after_chunk_without_newline() {
    let mut text = "x".repeat(CHUNK_SIZE + 40);
    text.push_str("\ntail\n");
    let expected = format_hashline_view("wide.txt", &text, Some(1), Some(2)).unwrap();
    let actual = format_window_bytes(
        text.as_bytes(),
        Some(1),
        Some(2),
        Some(FileHash::new()),
        hashline_header("wide.txt"),
    )
    .unwrap();
    assert_eq!(actual, expected);
}

// Covers: invalid UTF-8 must fail before a later valid window is decoded
// Owner: text view window
#[test]
fn window_rejects_invalid_utf8_before_selected_window() {
    let mut bytes = vec![b'a'; CHUNK_SIZE];
    bytes[10] = 0xFF;
    bytes.extend_from_slice(b"ok\nmore\n");
    let error = format_window_bytes(
        &bytes,
        Some(2),
        Some(1),
        Some(FileHash::new()),
        hashline_header("bad.txt"),
    )
    .unwrap_err();
    assert_eq!(error, ScanError::InvalidUtf8);
}

// Covers: disk scan path matches the in-memory split view on a multi-chunk file
// Owner: text view window
#[tokio::test]
async fn disk_window_matches_split_view() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.txt");
    let text = numbered_log(9_000, 40);
    std::fs::write(&path, &text).unwrap();
    let source_len = std::fs::metadata(&path).unwrap().len();
    assert!(source_len as usize > CHUNK_SIZE);

    let expected = format_hashline_view("log.txt", &text, Some(8_500), Some(4)).unwrap();
    let actual = read_text_window(
        &path,
        source_len,
        Some(8_500),
        Some(4),
        Some(FileHash::new()),
        hashline_header("log.txt"),
    )
    .await
    .unwrap();
    assert_eq!(actual, expected);
}
