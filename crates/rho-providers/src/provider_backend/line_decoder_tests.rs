use super::{LineDecodeError, LineDecoder, MaxLineBytes};

const BOUNDED_LIMIT: usize = 64;

fn bounded() -> LineDecoder {
    LineDecoder::with_max_line_bytes(BOUNDED_LIMIT)
}

#[test]
fn decodes_lf_crlf_empty_lines_and_trailing_tail_across_chunks() {
    let mut decoder = LineDecoder::default();
    let mut lines = Vec::new();

    for chunk in [
        &b"first\r"[..],
        &b"\nsecond\n\nmultibyte: \xc3"[..],
        &b"\xa9\r\ntail\r"[..],
    ] {
        decoder.push(chunk);
        while let Some(line) = decoder.next_line().unwrap() {
            lines.push(line.to_string());
        }
    }
    if let Some(line) = decoder.finish().unwrap() {
        lines.push(line.to_string());
    }

    assert_eq!(lines, ["first", "second", "", "multibyte: é", "tail"]);
}

#[test]
fn retains_only_an_incomplete_line_when_appending_a_chunk() {
    let mut decoder = LineDecoder::default();
    decoder.push(b"one\ntwo\npar");
    assert_eq!(decoder.next_line().unwrap(), Some("one"));
    assert_eq!(decoder.next_line().unwrap(), Some("two"));
    assert_eq!(decoder.next_line().unwrap(), None);

    decoder.push(b"tial\n");

    assert_eq!(decoder.next_line().unwrap(), Some("partial"));
    assert_eq!(decoder.finish().unwrap(), None);
}

#[test]
fn waits_for_a_complete_multibyte_character() {
    let mut decoder = LineDecoder::default();
    decoder.push(b"data: \xc3");
    assert_eq!(decoder.next_line().unwrap(), None);

    decoder.push(b"\xa9\n");

    assert_eq!(decoder.next_line().unwrap(), Some("data: é"));
}

#[test]
fn rejects_invalid_utf8_in_complete_lines_and_tail() {
    let mut line_decoder = LineDecoder::default();
    line_decoder.push(b"data: \xff\n");
    let line_error = line_decoder
        .next_line()
        .expect_err("complete invalid utf-8");
    assert!(matches!(line_error, LineDecodeError::InvalidUtf8(_)));
    assert!(
        line_error
            .to_string()
            .contains("invalid UTF-8 in stream line"),
        "{line_error}"
    );

    let mut tail_decoder = LineDecoder::default();
    tail_decoder.push(b"data: \xff");
    let tail_error = tail_decoder.finish().expect_err("finish invalid utf-8");
    assert!(matches!(tail_error, LineDecodeError::InvalidUtf8(_)));
    assert!(
        tail_error
            .to_string()
            .contains("invalid UTF-8 in stream line"),
        "{tail_error}"
    );
}

#[test]
fn unlimited_default_accepts_large_lines() {
    let mut decoder = LineDecoder::default();
    let large = vec![b'a'; BOUNDED_LIMIT * 8];
    decoder.push(&large);
    decoder.push(b"\n");
    let line = decoder
        .next_line()
        .expect("unlimited must accept large lines")
        .expect("line present");
    assert_eq!(line.len(), large.len());
    assert_eq!(decoder.finish().unwrap(), None);
}

#[test]
fn constructors_are_self_documenting() {
    assert_eq!(
        LineDecoder::unlimited().max_line_bytes_for_test(),
        MaxLineBytes::Unlimited
    );
    assert_eq!(
        LineDecoder::default().max_line_bytes_for_test(),
        MaxLineBytes::Unlimited
    );
    assert_eq!(
        LineDecoder::with_max_line_bytes(BOUNDED_LIMIT).max_line_bytes_for_test(),
        MaxLineBytes::Limited(BOUNDED_LIMIT)
    );
    assert_eq!(
        LineDecoder::new(MaxLineBytes::limited(BOUNDED_LIMIT)).max_line_bytes_for_test(),
        MaxLineBytes::Limited(BOUNDED_LIMIT)
    );
}

#[test]
fn rejects_complete_oversize_line_before_returning_it() {
    let mut decoder = bounded();
    let oversize = BOUNDED_LIMIT + 8;
    decoder.push(&vec![b'a'; oversize]);
    decoder.push(b"\n");
    let error = decoder
        .next_line()
        .expect_err("complete oversize line must fail");
    assert!(matches!(
        error,
        LineDecodeError::LineTooLong {
            limit: BOUNDED_LIMIT,
            ..
        }
    ));
    assert!(error.to_string().contains("exceeds"));
}

#[test]
fn accepts_complete_line_at_exact_byte_limit() {
    // Boundary: a line of exactly the configured limit must still be accepted.
    let mut decoder = bounded();
    decoder.push(&[b'a'; BOUNDED_LIMIT]);
    decoder.push(b"\n");
    let line = decoder
        .next_line()
        .expect("exact limit is in-bounds")
        .expect("line present");
    assert_eq!(line.len(), BOUNDED_LIMIT);
    assert!(line.bytes().all(|byte| byte == b'a'));
    assert_eq!(decoder.finish().unwrap(), None);
}

#[test]
fn rejects_unterminated_oversize_line_without_unbounded_growth() {
    let mut decoder = bounded();
    decoder.push(&[b'b'; BOUNDED_LIMIT]);
    // One more byte tips the incomplete tail over the cap.
    decoder.push(b"x");
    // Further bytes are ignored while the oversize error is still pending.
    decoder.push(&[b'y'; 64]);
    let error = decoder
        .next_line()
        .expect_err("unterminated oversize must fail on next_line");
    assert!(matches!(
        error,
        LineDecodeError::LineTooLong {
            limit: BOUNDED_LIMIT,
            ..
        }
    ));
    assert_eq!(decoder.finish().unwrap(), None);
}

#[test]
fn finish_rejects_unterminated_oversize_tail_when_error_pending() {
    let mut decoder = bounded();
    decoder.push(&[b'c'; BOUNDED_LIMIT + 1]);
    let error = decoder.finish().expect_err("finish must surface oversize");
    assert!(matches!(
        error,
        LineDecodeError::LineTooLong {
            limit: BOUNDED_LIMIT,
            ..
        }
    ));
}

#[test]
fn drains_valid_complete_lines_before_oversize_error() {
    let mut decoder = bounded();
    decoder.push(b"ok\n");
    decoder.push(&[b'z'; BOUNDED_LIMIT + 1]);
    assert_eq!(decoder.next_line().unwrap(), Some("ok"));
    let error = decoder.next_line().expect_err("oversize after valid line");
    assert!(matches!(error, LineDecodeError::LineTooLong { .. }));
}

#[test]
fn accepts_near_limit_payload_then_rejects_oversize() {
    let payload = "y".repeat(BOUNDED_LIMIT - 8);
    assert!(payload.len() < BOUNDED_LIMIT);

    let mut decoder = bounded();
    decoder.push(payload.as_bytes());
    decoder.push(b"\n");
    let line = decoder
        .next_line()
        .expect("near-limit payload is legitimate")
        .expect("line present");
    assert_eq!(line, payload);

    decoder.push(&[b'z'; BOUNDED_LIMIT + 1]);
    decoder.push(b"\n");
    let error = decoder
        .next_line()
        .expect_err("oversize after valid large line");
    assert!(matches!(
        error,
        LineDecodeError::LineTooLong {
            limit: BOUNDED_LIMIT,
            ..
        }
    ));
}

impl LineDecoder {
    fn max_line_bytes_for_test(&self) -> MaxLineBytes {
        self.max_line_bytes
    }
}
