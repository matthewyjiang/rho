use super::{LineDecodeError, LineDecoder, MAX_NDJSON_LINE_BYTES};

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
            .contains("invalid UTF-8 in stream-json line"),
        "{line_error}"
    );

    let mut tail_decoder = LineDecoder::default();
    tail_decoder.push(b"data: \xff");
    let tail_error = tail_decoder.finish().expect_err("finish invalid utf-8");
    assert!(matches!(tail_error, LineDecodeError::InvalidUtf8(_)));
    assert!(
        tail_error
            .to_string()
            .contains("invalid UTF-8 in stream-json line"),
        "{tail_error}"
    );
}

#[test]
fn rejects_complete_oversize_line_before_returning_it() {
    let mut decoder = LineDecoder::default();
    let oversize = MAX_NDJSON_LINE_BYTES + 8;
    decoder.push(&vec![b'a'; oversize]);
    decoder.push(b"\n");
    let error = decoder
        .next_line()
        .expect_err("complete oversize line must fail");
    assert!(matches!(
        error,
        LineDecodeError::LineTooLong {
            limit: MAX_NDJSON_LINE_BYTES,
            ..
        }
    ));
    assert!(error.to_string().contains("exceeds"));
}

#[test]
fn rejects_unterminated_oversize_line_without_unbounded_growth() {
    let mut decoder = LineDecoder::default();
    decoder.push(&vec![b'b'; MAX_NDJSON_LINE_BYTES]);
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
            limit: MAX_NDJSON_LINE_BYTES,
            ..
        }
    ));
    assert_eq!(decoder.finish().unwrap(), None);
}

#[test]
fn finish_rejects_unterminated_oversize_tail_when_error_pending() {
    let mut decoder = LineDecoder::default();
    decoder.push(&vec![b'c'; MAX_NDJSON_LINE_BYTES + 1]);
    let error = decoder.finish().expect_err("finish must surface oversize");
    assert!(matches!(
        error,
        LineDecodeError::LineTooLong {
            limit: MAX_NDJSON_LINE_BYTES,
            ..
        }
    ));
}

#[test]
fn drains_valid_complete_lines_before_oversize_error() {
    let mut decoder = LineDecoder::default();
    decoder.push(b"ok\n");
    decoder.push(&vec![b'z'; MAX_NDJSON_LINE_BYTES + 1]);
    assert_eq!(decoder.next_line().unwrap(), Some("ok"));
    let error = decoder.next_line().expect_err("oversize after valid line");
    assert!(matches!(error, LineDecodeError::LineTooLong { .. }));
}
