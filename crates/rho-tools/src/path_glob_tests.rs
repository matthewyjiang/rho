use pretty_assertions::assert_eq;

use super::PathGlob;
use crate::tool::ToolError;

// Covers: invalid globs (including a bare `!`) fail and name the bad pattern
// Owner: pure unit (shared grep/glob path filter)
#[test]
fn invalid_patterns_name_the_glob() {
    for (pattern, named) in [("a[", "a["), ("!", "'!'")] {
        let Err(ToolError::Message(message)) = PathGlob::compile(pattern) else {
            panic!("expected invalid glob for {pattern}");
        };
        assert!(message.contains(named), "{pattern}: {message}");
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
