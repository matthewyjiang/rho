use super::*;

/// The dialect's matching semantics where ECMAScript, the host engine
/// and Unicode disagree, beyond the conformance suite's cases.
#[test]
fn matching_semantics() {
    let cases = [
        (r"\bé", "é", false),
        (r"a\B", "ab", true),
        (r"^[^\s\S]$", "a", false),
        (r"^\s$", "\u{3000}", true),
        (r"^.$", "\u{2028}", false),
        (r"^[\w-]+$", "a-b_c", true),
        (r"^(?!abc).*$", "abd", true),
        (r"^\x41B\u{43}\/$", "ABC/", true),
        (r"^a{2,3}$", "aaaa", false),
    ];
    for (pattern, value, want) in cases {
        let pattern_ok = Pattern::new(pattern).unwrap_or_else(|e| panic!("{pattern}: {e}"));
        assert_eq!(
            pattern_ok.is_match(value),
            Ok(want),
            "{pattern} on {value:?}"
        );
    }
}
