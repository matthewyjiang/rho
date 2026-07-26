use pretty_assertions::assert_eq;

use super::PathGlob;
use crate::tool::ToolError;

#[test]
fn bare_extension_pattern_matches_nested_files() {
    let glob = PathGlob::compile("*.rs").unwrap();
    assert!(glob.matches("src/main.rs"));
    assert!(glob.matches("main.rs"));
    assert!(!glob.matches("src/main.toml"));
}

#[test]
fn single_segment_star_respects_literal_separator() {
    let glob = PathGlob::compile("src/*.rs").unwrap();
    assert!(glob.matches("src/main.rs"));
    assert!(!glob.matches("src/a/b.rs"));
}

#[test]
fn double_star_matches_any_depth() {
    let any_rs = PathGlob::compile("**/*.rs").unwrap();
    assert!(any_rs.matches("main.rs"));
    assert!(any_rs.matches("src/main.rs"));
    assert!(any_rs.matches("crates/rho/src/lib.rs"));

    let under_prefix = PathGlob::compile("crates/rho/**").unwrap();
    assert!(under_prefix.matches("crates/rho/src/lib.rs"));
    assert!(under_prefix.matches("crates/rho/Cargo.toml"));
    assert!(!under_prefix.matches("crates/rho-tools/src/lib.rs"));
}

#[test]
fn invalid_pattern_names_the_glob() {
    let Err(error) = PathGlob::compile("a[") else {
        panic!("expected invalid glob");
    };
    match error {
        ToolError::Message(message) => {
            assert!(message.contains("invalid glob 'a['"), "{message}");
        }
        other => panic!("expected Message, got {other:?}"),
    }
}

#[test]
fn matches_is_stable_for_compiled_pattern() {
    let glob = PathGlob::compile("src/**/*.rs").unwrap();
    assert_eq!(glob.matches("src/a.rs"), true);
    assert_eq!(glob.matches("lib/a.rs"), false);
}
