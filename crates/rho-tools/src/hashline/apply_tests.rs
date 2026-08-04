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
        &[
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
    assert_eq!(outcome.old_tag, tag);
    assert_eq!(outcome.new_tag, compute_file_hash(&outcome.text));
}

// Covers: stale tags must fail closed before mutating content
// Owner: hashline apply
#[test]
fn rejects_stale_tag() {
    let err = apply_ops("hello\n", "DEAD", &[Op::Delete { start: 1, end: 1 }]).unwrap_err();
    assert!(err.contains("tag mismatch"), "{err}");
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
        &[
            Op::Replace {
                start: 1,
                end: 2,
                body: vec!["x".into()],
            },
            Op::Delete { start: 2, end: 3 },
        ],
    )
    .unwrap_err();
    assert!(err.contains("overlapping"), "{err}");
}
