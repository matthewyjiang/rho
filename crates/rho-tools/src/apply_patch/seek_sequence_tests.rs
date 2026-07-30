use pretty_assertions::assert_eq;

use super::seek_sequence;

fn to_vec(strings: &[&str]) -> Vec<String> {
    strings.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn exact_match_finds_sequence() {
    let lines = to_vec(&["foo", "bar", "baz"]);
    let pattern = to_vec(&["bar", "baz"]);
    assert_eq!(
        seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ false),
        Some(1)
    );
}

#[test]
fn rstrip_match_ignores_trailing_whitespace() {
    let lines = to_vec(&["foo   ", "bar\t\t"]);
    let pattern = to_vec(&["foo", "bar"]);
    assert_eq!(
        seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ false),
        Some(0)
    );
}

#[test]
fn eof_prefers_final_window_then_falls_back() {
    let lines = to_vec(&["match", "middle", "match"]);
    let pattern = to_vec(&["match"]);
    assert_eq!(
        seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ true),
        Some(2)
    );
    assert_eq!(
        seek_sequence(&lines, &pattern, /*start*/ 1, /*eof*/ true),
        Some(2)
    );
    // No tail match: fall back from start.
    let lines = to_vec(&["match", "middle", "end"]);
    assert_eq!(
        seek_sequence(&lines, &pattern, /*start*/ 0, /*eof*/ true),
        Some(0)
    );
}
