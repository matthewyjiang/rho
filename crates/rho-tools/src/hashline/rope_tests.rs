use pretty_assertions::assert_eq;

use super::{format_hashline_view_bytes, read_hashline_window, CHUNK_SIZE};
use crate::hashline::format_hashline_view;

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

// Covers: request-local scan must match the split view, including a line that
// starts before a 256 KiB chunk boundary
// Owner: hashline rope
#[test]
fn rope_matches_split_across_chunk_windows() {
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
        let actual = format_hashline_view_bytes("log.txt", text.as_bytes(), offset, limit).unwrap();
        assert_eq!(actual, expected, "offset={offset:?} limit={limit:?}");
    }
}

// Covers: a line that continues past a chunk with no extra newline still hashes
// Owner: hashline rope
#[test]
fn rope_reads_partial_line_after_chunk_without_newline() {
    let mut text = "x".repeat(CHUNK_SIZE + 40);
    text.push_str("\ntail\n");
    let expected = format_hashline_view("wide.txt", &text, Some(1), Some(2)).unwrap();
    let actual = format_hashline_view_bytes("wide.txt", text.as_bytes(), Some(1), Some(2)).unwrap();
    assert_eq!(actual, expected);
}

// Covers: invalid UTF-8 must fail before a later valid window is decoded
// Owner: hashline rope
#[test]
fn rope_rejects_invalid_utf8_before_selected_window() {
    let mut bytes = vec![b'a'; CHUNK_SIZE];
    bytes[10] = 0xFF;
    bytes.extend_from_slice(b"ok\nmore\n");
    let error = format_hashline_view_bytes("bad.txt", &bytes, Some(2), Some(1)).unwrap_err();
    assert_eq!(error, "file is not valid UTF-8 text");
}

// Covers: disk scan path matches the in-memory split view on a multi-chunk file
// Owner: hashline rope
#[tokio::test]
async fn disk_window_matches_split_view() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.txt");
    let text = numbered_log(9_000, 40);
    std::fs::write(&path, &text).unwrap();
    let source_len = std::fs::metadata(&path).unwrap().len();
    assert!(source_len as usize > CHUNK_SIZE);

    let expected = format_hashline_view("log.txt", &text, Some(8_500), Some(4)).unwrap();
    let actual = read_hashline_window(
        &path,
        "log.txt",
        source_len,
        Some(8_500),
        Some(4),
        /*mint_tag*/ true,
    )
    .await
    .unwrap();
    assert_eq!(actual, expected);
}

// Covers: large-log pagination keeps only the selected window vs the previous split
// Owner: hashline rope
#[test]
#[ignore = "optimized large-log read benchmark; run with CARGO_PROFILE_TEST_OPT_LEVEL=3"]
fn large_log_read_benchmark() {
    use std::hint::black_box;

    let samples = std::env::var("RHO_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(5)
        .max(3);
    let line_count = 200_000;
    let text = numbered_log(line_count, 40);
    let bytes = text.as_bytes();
    let offset = Some(line_count - 200);
    let limit = Some(125);
    assert!(
        bytes.len() > CHUNK_SIZE * 2,
        "benchmark fixture must span several chunks, got {}",
        bytes.len()
    );

    let expected = format_hashline_view("bench.log", &text, offset, limit).unwrap();
    assert_eq!(
        format_hashline_view_bytes("bench.log", bytes, offset, limit).unwrap(),
        expected
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bench.log");
    std::fs::write(&path, &text).unwrap();
    let source_len = std::fs::metadata(&path).unwrap().len();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(read_hashline_window(
                &path,
                "bench.log",
                source_len,
                offset,
                limit,
                /*mint_tag*/ true,
            ))
            .unwrap(),
        expected
    );

    let previous = measure(samples, || {
        black_box(format_hashline_view("bench.log", &text, offset, limit).unwrap())
    });
    let rope = measure(samples, || {
        black_box(format_hashline_view_bytes("bench.log", bytes, offset, limit).unwrap())
    });
    let disk = measure(samples, || {
        black_box(
            runtime
                .block_on(read_hashline_window(
                    &path,
                    "bench.log",
                    source_len,
                    offset,
                    limit,
                    /*mint_tag*/ true,
                ))
                .unwrap(),
        )
    });

    eprintln!(
        "large-log {line_count} lines / {} bytes, offset {} limit 125, {samples} samples\n\
         previous split: median {} ms\n\
         in-memory scan: median {} ms ({:.1}x)\n\
         disk scan:      median {} ms ({:.1}x)",
        bytes.len(),
        line_count - 200,
        previous / 1_000_000,
        rope / 1_000_000,
        previous as f64 / rope.max(1) as f64,
        disk / 1_000_000,
        previous as f64 / disk.max(1) as f64,
    );
}

fn measure<T>(samples: usize, mut operation: impl FnMut() -> T) -> u64 {
    use std::time::Instant;

    for _ in 0..2 {
        let _ = operation();
    }
    let mut times = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let _ = operation();
        times.push(started.elapsed().as_nanos() as u64);
    }
    times.sort_unstable();
    times[times.len() / 2]
}
