use pretty_assertions::assert_eq;

use super::{read_text_window, scan_window_bytes, select_window, ScanError, CHUNK_SIZE};

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

// Covers: split and chunked selection agree on content bytes and line
// boundaries, and the numbered render keeps them
// Owner: text view
#[test]
fn numbered_renderers_preserve_line_boundaries() {
    let cases = [
        ("", "file.txt"),
        ("\n", "file.txt\n1:"),
        ("one", "file.txt\n1:one"),
        ("one\n", "file.txt\n1:one"),
        ("one\n\n", "file.txt\n1:one\n2:"),
        ("α\r\n🦀\r\n", "file.txt\n1:α\n2:🦀"),
        ("α\r\n🦀", "file.txt\n1:α\n2:🦀"),
    ];
    for (text, expected) in cases {
        let split = select_window(text, None, None).unwrap();
        let scanned = scan_window_bytes(text.as_bytes(), None, None).unwrap();
        assert_eq!(scanned, split, "scanned input={text:?}");
        assert_eq!(split.render("file.txt"), expected, "input={text:?}");
    }
}

// Covers: request-local scan must match the split view, including a line that
// starts before a 256 KiB chunk boundary or continues past one
// Owner: text view window
#[test]
fn window_matches_split_across_chunk_windows() {
    let (crossing, crossing_line) = crossing_chunk_text();
    let log = numbered_log(8_000, 40);
    let no_nl = format!("{}tail", "x".repeat(CHUNK_SIZE + 10));
    let wide = format!("{}\ntail\n", "x".repeat(CHUNK_SIZE + 40));
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
        (wide.as_str(), Some(1), Some(2)),
    ];
    for (text, offset, limit) in cases {
        let expected = select_window(text, offset, limit).unwrap();
        let actual = scan_window_bytes(text.as_bytes(), offset, limit).unwrap();
        assert_eq!(actual, expected, "offset={offset:?} limit={limit:?}");
    }
}

// Covers: invalid UTF-8 must fail before a later valid window is decoded
// Owner: text view window
#[test]
fn window_rejects_invalid_utf8_before_selected_window() {
    let mut bytes = vec![b'a'; CHUNK_SIZE];
    bytes[10] = 0xFF;
    bytes.extend_from_slice(b"ok\nmore\n");
    let error = scan_window_bytes(&bytes, Some(2), Some(1)).unwrap_err();
    assert_eq!(error, ScanError::InvalidUtf8);
}

// Covers: large ranged disk reads must match the in-memory numbered view
// Owner: text view window
#[tokio::test]
async fn numbered_disk_window_matches_split_view() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.txt");
    let text = numbered_log(9_000, 40);
    std::fs::write(&path, &text).unwrap();
    let source_len = std::fs::metadata(&path).unwrap().len();
    assert!(source_len as usize > CHUNK_SIZE);

    let cases = [
        (Some(1), Some(4)),
        (Some(4_500), Some(6)),
        (Some(8_990), Some(20)),
    ];
    for (offset, limit) in cases {
        let expected = select_window(&text, offset, limit).unwrap();
        let actual = read_text_window(&path, source_len, offset, limit)
            .await
            .unwrap();
        assert_eq!(actual, expected, "offset={offset:?} limit={limit:?}");
    }
}
