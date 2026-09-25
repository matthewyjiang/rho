use pretty_assertions::assert_eq;

use super::PathGlob;
use crate::tool::ToolError;

#[test]
fn invalid_pattern_names_the_glob() {
    let Err(error) = PathGlob::compile("a[") else {
        panic!("expected invalid glob");
    };
    match error {
        ToolError::Message(message) => {
            assert!(message.contains("a["), "{message}");
        }
        other => panic!("expected Message, got {other:?}"),
    }
}

// Covers: a leading `!` excludes matches like `rg -g '!…'` instead of
// compiling to a literal-`!` glob that silently matches nothing
// Owner: pure unit (shared grep/glob path filter)
#[test]
fn leading_bang_inverts_the_match() {
    for (pattern, path, expected) in [
        ("!*_tests.rs", "src/image_input.rs", true),
        ("!*_tests.rs", "src/image_input_tests.rs", false),
        ("!*_tests.rs", "image_input_tests.rs", false),
        ("!src/*.rs", "src/lib.rs", false),
        ("!src/*.rs", "tests/lib.rs", true),
        ("*_tests.rs", "src/image_input_tests.rs", true),
    ] {
        let glob = PathGlob::compile(pattern).unwrap();
        assert_eq!(glob.matches(path), expected, "{pattern} vs {path}");
    }
}

#[test]
fn bare_bang_is_rejected() {
    let Err(ToolError::Message(message)) = PathGlob::compile("!") else {
        panic!("expected invalid glob");
    };
    assert!(message.contains("'!'"), "{message}");
}
