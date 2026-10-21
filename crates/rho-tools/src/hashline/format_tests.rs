use pretty_assertions::assert_eq;

use super::*;

// Covers: snapshot tags must stay stable for equivalent normalized text
// Owner: hashline format
#[test]
fn file_hash_ignores_trailing_spaces_and_crlf() {
    let lf = "fn main() {\n    ok();\n}\n";
    let crlf_spaces = "fn main() {\r\n    ok();   \r\n}\r\n";
    assert_eq!(compute_file_hash(lf), compute_file_hash(crlf_spaces));
    assert_eq!(
        compute_file_hash(lf),
        compute_file_hash_bytes(lf.as_bytes())
    );
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

// Covers: post-edit previews must expose the new tag and renumbered focus lines
// Owner: hashline format
#[test]
fn post_edit_preview_uses_new_tag_and_post_edit_line_numbers() {
    let new = "one\nTWO\nthree\nfour\nfive\n";
    let preview = format_post_edit_preview("sample.txt", new, &[2], /*structural*/ false);
    let new_tag = compute_file_hash(new);
    assert!(
        preview.starts_with(&format!("[sample.txt#{new_tag}]\n")),
        "{preview}"
    );
    assert!(preview.contains("2:TWO"), "{preview}");
    assert!(!preview.contains("2:two"), "{preview}");
}

// Covers: pure deletes still produce a usable numbered anchor near the gap
// Owner: hashline format
#[test]
fn post_edit_preview_keeps_context_around_deletes() {
    let new = "a\nd\n";
    let preview = format_post_edit_preview("x.txt", new, &[1, 2], /*structural*/ false);
    let new_tag = compute_file_hash(new);
    assert!(
        preview.starts_with(&format!("[x.txt#{new_tag}]\n")),
        "{preview}"
    );
    assert!(preview.contains("1:a"), "{preview}");
    assert!(preview.contains("2:d"), "{preview}");
}

// Covers: multi-hunk previews must not starve later focus regions under the body cap
// Owner: hashline format
#[test]
fn post_edit_preview_spreads_budget_across_hunks() {
    let mut lines = Vec::new();
    for i in 1..=80 {
        lines.push(format!("line-{i}"));
    }
    let new = format!("{}\n", lines.join("\n"));
    // Two distant focus points; with a tiny budget both hunks must appear.
    let preview = format_post_edit_preview("big.txt", &new, &[2, 70], /*structural*/ false);
    assert!(preview.contains("2:line-2"), "{preview}");
    assert!(preview.contains("70:line-70"), "{preview}");
}

// Covers: structural edits omit chainable body lines and force re-read
// Owner: hashline format
#[test]
fn post_edit_preview_marks_structural_edits() {
    let new = "only\n";
    let preview = format_post_edit_preview("x.txt", new, &[1], /*structural*/ true);
    let tag = compute_file_hash(new);
    assert!(preview.starts_with(&format!("[x.txt#{tag}]")), "{preview}");
    assert!(
        preview.contains(STRUCTURAL_EDIT_FOOTER_MARKER),
        "expected structural footer: {preview}"
    );
    assert!(preview.contains(STRUCTURAL_NO_CHAIN_MARKER), "{preview}");
    assert!(
        !preview.contains("1:only"),
        "structural preview must not expose numbered body lines: {preview}"
    );
}
