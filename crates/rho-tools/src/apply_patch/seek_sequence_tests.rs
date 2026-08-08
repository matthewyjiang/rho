use pretty_assertions::assert_eq;

use super::seek_sequence;

fn lines(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

// Covers: context matching tolerates model whitespace drift and EOF anchors prefer the tail.
// Owner: apply_patch context matching
#[test]
fn matches_normalized_context_and_eof_position() {
    let whitespace_source = lines(&["first", "value   "]);

    assert_eq!(
        seek_sequence(
            &whitespace_source,
            &lines(&["value"]),
            /*start*/ 0,
            /*eof*/ false
        ),
        Some(1)
    );
    let repeated_source = lines(&["value", "middle", "value"]);
    assert_eq!(
        seek_sequence(
            &repeated_source,
            &lines(&["value"]),
            /*start*/ 0,
            /*eof*/ true
        ),
        Some(2)
    );
}
