use pretty_assertions::assert_eq;

use super::seek_sequence;

fn lines(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

// Covers: context matching handles misses, whitespace drift, Unicode punctuation, and EOF anchors.
// Owner: apply_patch context matching
#[test]
fn matches_context_by_normalization_rule() {
    let cases = [
        (
            "trailing whitespace",
            vec!["first", "value   "],
            "value",
            false,
            Some(1),
        ),
        ("no match", vec!["first", "value"], "missing", false, None),
        (
            "smart quote",
            vec!["say ‘hello’"],
            "say 'hello'",
            false,
            Some(0),
        ),
        (
            "unicode dash",
            vec!["alpha—beta"],
            "alpha-beta",
            false,
            Some(0),
        ),
        (
            "EOF prefers tail",
            vec!["value", "middle", "value"],
            "value",
            true,
            Some(2),
        ),
    ];

    for (name, source, pattern, eof, expected) in cases {
        assert_eq!(
            seek_sequence(&lines(&source), &lines(&[pattern]), /*start*/ 0, eof),
            expected,
            "{name}"
        );
    }
}
