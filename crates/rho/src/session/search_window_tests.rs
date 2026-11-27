use pretty_assertions::assert_eq;

use super::{window, Window, CLOSE, OPEN};

/// `text` with each of `words` marked as FTS5 `highlight()` marks a match.
fn marked(text: &str, words: &[&str]) -> String {
    words.iter().fold(text.to_owned(), |text, word| {
        text.replace(word, &format!("{OPEN}{word}{CLOSE}"))
    })
}

// Covers: a hit shows the part of a message around its first match, with
// byte ranges a client can slice `text` with to bold each match: on char
// boundaries, clipped to the window, and unmoved by marker characters the
// message itself holds. Owner: session search excerpts.
#[test]
// Highlights are a list of byte ranges; one match is a list of one.
#[allow(clippy::single_range_in_vec_init)]
fn a_window_shows_the_first_match_with_byte_highlights() {
    let long_lead = format!("{} needle", "a".repeat(100));
    let clipped = format!("needle{}needle tail", "x".repeat(312));
    let cases = [
        (
            "no match shows the start",
            "plain".to_owned(),
            "plain".to_owned(),
            Window {
                start: 0,
                end: 5,
                total_chars: 5,
                text: "plain".into(),
                highlights: vec![],
            },
        ),
        (
            "highlights are bytes, not chars",
            "é🚀 needle".to_owned(),
            marked("é🚀 needle", &["needle"]),
            Window {
                start: 0,
                end: 9,
                total_chars: 9,
                text: "é🚀 needle".into(),
                highlights: vec![7..13],
            },
        ),
        (
            "80 characters lead a later match",
            long_lead.clone(),
            marked(&long_lead, &["needle"]),
            Window {
                start: 21,
                end: 107,
                total_chars: 107,
                text: long_lead[21..].into(),
                highlights: vec![80..86],
            },
        ),
        (
            "a match the window cuts is clipped, one past it dropped",
            clipped.clone(),
            marked(&clipped, &["needle", "tail"]),
            Window {
                start: 0,
                end: 320,
                total_chars: 329,
                text: clipped[..320].into(),
                highlights: vec![0..6, 318..320],
            },
        ),
        (
            "marker characters in the message are text",
            format!("a{OPEN}b needle"),
            format!("a{OPEN}b {OPEN}needle{CLOSE}"),
            Window {
                start: 0,
                end: 10,
                total_chars: 10,
                text: format!("a{OPEN}b needle"),
                highlights: vec![4..10],
            },
        ),
        (
            "highlighted text of another message marks nothing",
            "abc".to_owned(),
            "xyz".to_owned(),
            Window {
                start: 0,
                end: 3,
                total_chars: 3,
                text: "abc".into(),
                highlights: vec![],
            },
        ),
    ];

    let actual: Vec<_> = cases
        .iter()
        .map(|(name, text, highlighted, _)| (*name, window(text, highlighted)))
        .collect();
    let expected: Vec<_> = cases
        .into_iter()
        .map(|(name, _, _, expected)| (name, expected))
        .collect();
    assert_eq!(actual, expected);
}
