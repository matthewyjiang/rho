use pretty_assertions::assert_eq;

use super::*;
use crate::hashline::{format::compute_file_hash, parser::Op};

// Covers: replace + insert + cut against original line numbers
// Owner: hashline apply
#[test]
fn applies_mixed_ops_on_original_line_numbers() {
    let original = "one\ntwo\nthree\nfour\n";
    let tag = compute_file_hash(original);
    let outcome = apply_ops(
        original,
        &tag,
        vec![
            Op::Replace {
                start: 2,
                end: 2,
                body: vec!["TWO".into()],
            },
            Op::InsertAfter {
                line: Some(3),
                body: vec!["3.5".into()],
            },
            Op::Delete { start: 4, end: 4 },
        ],
    )
    .unwrap();
    assert_eq!(outcome.text, "one\nTWO\nthree\n3.5\n");
    assert_eq!(outcome.focus_lines, vec![2, 4]);
    assert_eq!(
        compute_file_hash(&outcome.text),
        compute_file_hash("one\nTWO\nthree\n3.5\n")
    );
}

// Covers: stale tags must fail closed before mutating content
// Owner: hashline apply
#[test]
fn rejects_stale_tag() {
    let err = apply_ops("hello\n", "DEAD", vec![Op::Delete { start: 1, end: 1 }]).unwrap_err();
    assert!(
        matches!(
            err,
            ApplyError::TagMismatch {
                expected: _,
                live: _
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("tag mismatch"), "{err}");
}

// Covers: overlapping destructive ranges must fail closed
// Owner: hashline apply
#[test]
fn rejects_overlapping_replaces() {
    let original = "a\nb\nc\n";
    let tag = compute_file_hash(original);
    let err = apply_ops(
        original,
        &tag,
        vec![
            Op::Replace {
                start: 1,
                end: 2,
                body: vec!["x".into()],
            },
            Op::Delete { start: 2, end: 3 },
        ],
    )
    .unwrap_err();
    assert!(err.to_string().contains("overlapping"), "{err}");
}

// Covers: an insert anchored inside a replaced or deleted range must fail closed
// rather than be silently dropped from the output
// Owner: hashline apply
#[test]
fn rejects_inserts_anchored_inside_a_destructive_range() {
    let original = "a\nb\nc\nd\ne\n";
    let tag = compute_file_hash(original);
    let replace = || Op::Replace {
        start: 2,
        end: 4,
        body: vec!["B".into()],
    };
    for (label, extra) in [
        (
            "insert after",
            Op::InsertAfter {
                line: Some(3),
                body: vec!["ghost".into()],
            },
        ),
        (
            "insert before",
            Op::InsertBefore {
                line: 3,
                body: vec!["ghost".into()],
            },
        ),
        (
            "insert after the anchor line of the range",
            Op::InsertAfter {
                line: Some(2),
                body: vec!["ghost".into()],
            },
        ),
    ] {
        let err = apply_ops(original, &tag, vec![replace(), extra]).unwrap_err();
        assert!(err.to_string().contains("falls inside"), "{label}: {err}");
    }
}

// Covers: inserts at the edges of a destructive range stay legal and ordered
// Owner: hashline apply
#[test]
fn keeps_inserts_at_range_edges() {
    let original = "a\nb\nc\nd\ne\n";
    let tag = compute_file_hash(original);
    let outcome = apply_ops(
        original,
        &tag,
        vec![
            Op::InsertBefore {
                line: 2,
                body: vec!["head".into()],
            },
            Op::Delete { start: 2, end: 4 },
            Op::InsertAfter {
                line: Some(5),
                body: vec!["tail".into()],
            },
        ],
    )
    .unwrap();
    assert_eq!(outcome.text, "a\nhead\ne\ntail\n");
    assert_eq!(outcome.focus_lines, vec![2, 3, 4]);
}

// Covers: multiple inserts on one anchor keep document order
// Owner: hashline apply
#[test]
fn keeps_document_order_for_shared_anchors() {
    let original = "a\nb\n";
    let tag = compute_file_hash(original);
    let outcome = apply_ops(
        original,
        &tag,
        vec![
            Op::InsertAfter {
                line: Some(1),
                body: vec!["first".into()],
            },
            Op::InsertAfter {
                line: Some(1),
                body: vec!["second".into()],
            },
        ],
    )
    .unwrap();
    assert_eq!(outcome.text, "a\nfirst\nsecond\nb\n");
    assert_eq!(outcome.focus_lines, vec![2, 3]);
}

// Covers: an empty file accepts head inserts and end-of-file appends only
// Owner: hashline apply
#[test]
fn appends_into_an_empty_file() {
    let tag = compute_file_hash("");
    let outcome = apply_ops(
        "",
        &tag,
        vec![
            Op::InsertBefore {
                line: 1,
                body: vec!["first".into()],
            },
            Op::InsertAfter {
                line: None,
                body: vec!["second".into()],
            },
        ],
    )
    .unwrap();
    assert_eq!(outcome.text, "first\nsecond\n");
    assert_eq!(outcome.focus_lines, vec![1, 2]);
}
