use pretty_assertions::assert_eq;

use super::*;

// Covers: a chatty child cannot grow the stderr capture without bound, and the
// kept slice is the tail, marked as elided.
// Owner: pure unit
#[test]
fn stderr_capture_keeps_a_bounded_tail() {
    let mut tail = StderrTail::default();
    for _ in 0..64 {
        tail.push(&[b'a'; MAX_STDERR_BYTES]);
        assert!(
            tail.bytes.len() <= MAX_STDERR_BYTES,
            "capture grew past its budget"
        );
    }
    tail.push(b"last line\n");

    let text = tail.finish();
    assert!(text.starts_with(rho_sdk::ELLIPSIS), "elision is not marked");
    assert!(
        text.ends_with("last line"),
        "kept the head instead of the tail"
    );
}

// Covers: cutting the head mid-character never opens the tail on a replacement
// character.
// Owner: pure unit
#[test]
fn stderr_capture_cuts_on_a_character_boundary() {
    let mut tail = StderrTail::default();
    // Three-byte characters make every cut land inside one unless it is walked
    // forward: the budget is not a multiple of three.
    tail.push("★".repeat(MAX_STDERR_BYTES).as_bytes());

    let text = tail.finish();
    assert_eq!(text.matches('\u{FFFD}').count(), 0);
}

// Covers: stderr short enough to keep whole is reported without an elision
// marker.
// Owner: pure unit
#[test]
fn stderr_capture_keeps_short_output_whole() {
    let mut tail = StderrTail::default();
    tail.push(b"  boom\n");

    assert_eq!(tail.finish(), "boom");
}
