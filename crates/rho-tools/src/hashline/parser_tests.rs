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
