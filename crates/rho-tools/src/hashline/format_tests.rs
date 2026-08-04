use pretty_assertions::assert_eq;

use super::*;

// Covers: empty and trailing-newline files must not invent phantom content lines
// Owner: hashline format
#[test]
fn split_content_lines_handles_empty_and_trailing_newline() {
    assert!(split_content_lines("").is_empty());
    assert_eq!(split_content_lines("a\nb"), vec!["a", "b"]);
    assert_eq!(split_content_lines("a\nb\n"), vec!["a", "b"]);
    assert_eq!(split_content_lines("\n"), vec![""]);
    assert_eq!(split_content_lines("a\r\nb\r\n"), vec!["a", "b"]);
}

// Covers: snapshot tags must stay stable for equivalent normalized text
// Owner: hashline format
#[test]
fn file_hash_ignores_trailing_spaces_and_crlf() {
    let lf = "fn main() {\n    ok();\n}\n";
    let crlf_spaces = "fn main() {\r\n    ok();   \r\n}\r\n";
    assert_eq!(compute_file_hash(lf), compute_file_hash(crlf_spaces));
    assert_eq!(compute_file_hash(lf).len(), FILE_HASH_LENGTH);
    assert!(compute_file_hash(lf)
        .chars()
        .all(|ch| ch.is_ascii_hexdigit()));
}

// Covers: partial reads keep full-file tags and absolute line numbers
// Owner: hashline format
#[test]
fn hashline_view_uses_absolute_lines_and_full_file_tag() {
    let text = "one\ntwo\nthree\nfour\n";
    let view = format_hashline_view("src/a.rs", text, Some(2), Some(2)).unwrap();
    let hash = compute_file_hash(text);
    assert_eq!(
        view,
        format!(
            "[src/a.rs#{hash}]\n2:two\n3:three\n\n[lines 2-3 of 4 shown; re-read with a different offset or limit for the rest]"
        )
    );
}
