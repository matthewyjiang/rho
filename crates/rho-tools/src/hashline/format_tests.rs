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

// Covers: post-edit previews must expose the new tag and renumbered focus lines
// Owner: hashline format
#[test]
fn post_edit_preview_uses_new_tag_and_post_edit_line_numbers() {
    let new = "one\nTWO\nthree\nfour\nfive\n";
    let preview = format_post_edit_preview("sample.txt", new, &[2]);
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
    let preview = format_post_edit_preview("x.txt", new, &[1, 2]);
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
    let preview = format_post_edit_preview("big.txt", &new, &[2, 70]);
    assert!(preview.contains("2:line-2"), "{preview}");
    assert!(preview.contains("70:line-70"), "{preview}");
}

// Covers: provider TUI fixture hardcodes this tag for "original line\n"
// Owner: hashline format
#[test]
fn fixture_edit_original_tag_matches_compute_file_hash() {
    assert_eq!(compute_file_hash("original line\n"), "8022");
}

// Covers: chain snapshots stay bounded and keep head+tail for large files
// Owner: hashline format
#[test]
fn chain_snapshot_uses_head_tail_window_without_focus() {
    let mut lines = Vec::new();
    for i in 1..=80 {
        lines.push(format!("line-{i}"));
    }
    let text = format!("{}\n", lines.join("\n"));
    let snapshot = format_chain_snapshot("big.txt", &text, &[]);
    let tag = compute_file_hash(&text);
    assert!(
        snapshot.starts_with(&format!("[big.txt#{tag}]\n")),
        "{snapshot}"
    );
    assert!(snapshot.contains("1:line-1"), "{snapshot}");
    assert!(snapshot.contains("80:line-80"), "{snapshot}");
    assert!(snapshot.contains("…\n"), "{snapshot}");
    assert!(snapshot.contains("showing"), "{snapshot}");
    assert!(!snapshot.contains("40:line-40"), "{snapshot}");
}

// Covers: failure focus keeps anchors without dumping the whole file
// Owner: hashline format
#[test]
fn chain_snapshot_focuses_around_anchors() {
    let mut lines = Vec::new();
    for i in 1..=80 {
        lines.push(format!("line-{i}"));
    }
    let text = format!("{}\n", lines.join("\n"));
    let snapshot = format_chain_snapshot("big.txt", &text, &[50]);
    assert!(snapshot.contains("50:line-50"), "{snapshot}");
    assert!(!snapshot.contains("1:line-1"), "{snapshot}");
    assert!(!snapshot.contains("80:line-80"), "{snapshot}");
}
