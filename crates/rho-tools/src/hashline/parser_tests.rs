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

// Covers: unsupported features, truncated locators, and empty PUT bodies fail
// with actionable errors (not a generic integer error that hides a stray dot)
// Owner: hashline parser
#[test]
fn rejects_invalid_documents_with_actionable_errors() {
    for (case, input, expected, absent) in [
        (
            "block ops",
            "[a.rs#ABCD]\nPUT 1*:\n+x\n",
            &["block ops"][..],
            None,
        ),
        (
            "registers",
            "[a.rs#ABCD]\nCUT 1.=2 @name\n",
            &["registers"][..],
            None,
        ),
        (
            "truncated PUT range",
            "[a.rs#ABCD]\nPUT 3.:\n+only\n",
            &["truncated", "\"3.\"", "PUT 3:"][..],
            Some("positive integer"),
        ),
        (
            "incomplete PUT range",
            "[a.rs#ABCD]\nPUT 3.=:\n+only\n",
            &["incomplete", "\"3.=\""][..],
            None,
        ),
        (
            "truncated CUT range",
            "[a.rs#ABCD]\nCUT 3.\n",
            &["truncated", "\"3.\""][..],
            None,
        ),
        (
            "empty PUT body",
            "[a.rs#ABCD]\nPUT 1.=2:\n",
            &["use CUT to delete"][..],
            None,
        ),
    ] {
        let err = parse_hashline(input).unwrap_err();
        for fragment in expected {
            assert!(err.contains(fragment), "{case}: {err}");
        }
        if let Some(absent) = absent {
            assert!(!err.contains(absent), "{case}: {err}");
        }
    }
}

// Covers: single-line shorthand, lowercase keywords, dash ranges, and body-row
// trailing whitespace all parse to the exact ops
// Owner: hashline parser
#[test]
fn parses_shorthand_and_lenient_spellings() {
    for (case, input, expected) in [
        (
            "single-line PUT N: means N.=N",
            "[a.rs#ABCD]\nPUT 3:\n+only\n",
            vec![Op::Replace {
                start: 3,
                end: 3,
                body: vec!["only".into()],
            }],
        ),
        (
            "body rows keep trailing spaces and tabs",
            "[a.rs#ABCD]\nPUT 1:\n+keep trailing  \n+\tkeep tab\t\n",
            vec![Op::Replace {
                start: 1,
                end: 1,
                body: vec!["keep trailing  ".into(), "\tkeep tab\t".into()],
            }],
        ),
        (
            "lowercase keywords and dash ranges",
            "[a.rs#ABCD]\nput 1-2:\n+a\n+b\ncut 3-3\n",
            vec![
                Op::Replace {
                    start: 1,
                    end: 2,
                    body: vec!["a".into(), "b".into()],
                },
                Op::Delete { start: 3, end: 3 },
            ],
        ),
    ] {
        let sections = parse_hashline(input).unwrap();
        assert_eq!(sections[0].ops, expected, "{case}");
    }
}
