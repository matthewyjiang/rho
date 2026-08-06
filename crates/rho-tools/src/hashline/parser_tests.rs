use pretty_assertions::assert_eq;

use super::*;

// Covers: replace/insert/cut sections must parse into concrete ops
// Owner: hashline parser
#[test]
fn parses_core_ops() {
    let sections = parse_hashline(
        r#"[src/a.rs#A1B2]
PUT 1.=2:
+alpha
+beta
PUT >2:
+gamma
CUT 4.=4
PUT >$:
+tail
"#,
    )
    .unwrap();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].path, "src/a.rs");
    assert_eq!(sections[0].tag, "A1B2");
    assert_eq!(
        sections[0].ops,
        vec![
            Op::Replace {
                start: 1,
                end: 2,
                body: vec!["alpha".into(), "beta".into()],
            },
            Op::InsertAfter {
                line: Some(2),
                body: vec!["gamma".into()],
            },
            Op::Delete { start: 4, end: 4 },
            Op::InsertAfter {
                line: None,
                body: vec!["tail".into()],
            },
        ]
    );
}

// Covers: unsupported full-hashline features fail with actionable errors
// Owner: hashline parser
#[test]
fn rejects_unsupported_block_and_register_ops() {
    let err = parse_hashline("[a.rs#ABCD]\nPUT 1*:\n+x\n").unwrap_err();
    assert!(err.contains("block ops"), "{err}");
    let err = parse_hashline("[a.rs#ABCD]\nCUT 1.=2 @name\n").unwrap_err();
    assert!(err.contains("registers"), "{err}");
}

// Covers: bare single-line PUT N: is accepted as N.=N
// Owner: hashline parser
#[test]
fn accepts_single_line_put_shorthand() {
    let sections = parse_hashline("[a.rs#ABCD]\nPUT 3:\n+only\n").unwrap();
    assert_eq!(
        sections[0].ops,
        vec![Op::Replace {
            start: 3,
            end: 3,
            body: vec!["only".into()],
        }]
    );
}

// Covers: PUT N.: / N.= must fail with an explicit truncated-range diagnostic
// (not a generic "not a positive integer" that hides the stray dot)
// Owner: hashline parser
#[test]
fn rejects_truncated_range_locators_with_clear_errors() {
    let err = parse_hashline("[a.rs#ABCD]\nPUT 3.:\n+only\n").unwrap_err();
    assert!(err.contains("truncated"), "{err}");
    assert!(err.contains("\"3.\""), "{err}");
    assert!(err.contains("PUT 3:"), "{err}");
    assert!(!err.contains("positive integer"), "{err}");

    let err = parse_hashline("[a.rs#ABCD]\nPUT 3.=:\n+only\n").unwrap_err();
    assert!(err.contains("incomplete"), "{err}");
    assert!(err.contains("\"3.=\""), "{err}");

    let err = parse_hashline("[a.rs#ABCD]\nCUT 3.\n").unwrap_err();
    assert!(err.contains("truncated"), "{err}");
    assert!(err.contains("\"3.\""), "{err}");
}

// Covers: body rows keep trailing spaces/tabs; only a wire CR is stripped
// Owner: hashline parser
#[test]
fn preserves_trailing_spaces_and_tabs_in_body_rows() {
    let sections =
        parse_hashline("[a.rs#ABCD]\nPUT 1:\n+keep trailing  \n+\tkeep tab\t\n").unwrap();
    assert_eq!(
        sections[0].ops,
        vec![Op::Replace {
            start: 1,
            end: 1,
            body: vec!["keep trailing  ".into(), "\tkeep tab\t".into()],
        }]
    );
}

// Covers: empty PUT body is not a second delete opcode; use CUT
// Owner: hashline parser
#[test]
fn rejects_empty_put_body() {
    let err = parse_hashline("[a.rs#ABCD]\nPUT 1.=2:\n").unwrap_err();
    assert!(err.contains("use CUT to delete"), "{err}");
}

// Covers: streaming preview must survive a half-written document
// Owner: hashline parser
#[test]
fn parses_incomplete_documents_leniently() {
    let sections =
        parse_lenient("[a.rs#ABCD]\nPUT 1.=3:\n+one\n+two\nCUT 8.=9\n[b.rs#ABCD]\nPUT 2:\n+pa");
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].ops.len(), 2);
    assert_eq!(sections[1].ops.len(), 1);
}

// Covers: model-friendly range/keyword spellings must still parse
// Owner: hashline parser
#[test]
fn accepts_lenient_range_and_keyword_case() {
    let sections = parse_hashline("[a.rs#ABCD]\nput 1-2:\n+a\n+b\ncut 3-3\n").unwrap();
    assert_eq!(
        sections[0].ops,
        vec![
            Op::Replace {
                start: 1,
                end: 2,
                body: vec!["a".into(), "b".into()],
            },
            Op::Delete { start: 3, end: 3 },
        ]
    );
}
