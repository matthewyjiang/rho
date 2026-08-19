use pretty_assertions::assert_eq;

use super::*;

// Covers: empty and trailing-newline files must not invent phantom content lines
// Owner: text view
#[test]
fn split_content_lines_handles_empty_and_trailing_newline() {
    assert!(split_content_lines("").is_empty());
    assert_eq!(split_content_lines("a\nb"), vec!["a", "b"]);
    assert_eq!(split_content_lines("a\nb\n"), vec!["a", "b"]);
    assert_eq!(split_content_lines("\n"), vec![""]);
    assert_eq!(split_content_lines("a\r\nb\r\n"), vec!["a", "b"]);

    assert_eq!(
        iter_content_lines("").collect::<Vec<_>>(),
        Vec::<&str>::new()
    );
    assert_eq!(
        iter_content_lines("a\nb").collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(
        iter_content_lines("a\nb\n").collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(iter_content_lines("\n").collect::<Vec<_>>(), vec![""]);
    assert_eq!(
        iter_content_lines("a\r\nb\r\n").collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

// Covers: numbered reads keep absolute line numbers without a snapshot tag
// Owner: text view
#[test]
fn numbered_view_uses_absolute_lines_and_path_header() {
    let text = "one\ntwo\nthree\nfour\n";
    let view = format_numbered_view("src/a.rs", text, Some(2), Some(2)).unwrap();
    assert_eq!(
        view,
        "src/a.rs\n2:two\n3:three\n\n[lines 2-3 of 4 shown; re-read with a different offset or limit for the rest]"
    );
}

// Covers: chain snapshots stay bounded and keep head+tail for large files
// Owner: text view
#[test]
fn chain_snapshot_uses_head_tail_window_without_focus() {
    let mut lines = Vec::new();
    for i in 1..=80 {
        lines.push(format!("line-{i}"));
    }
    let text = format!("{}\n", lines.join("\n"));
    let snapshot = format_chain_snapshot("big.txt", &text, &[]);
    assert!(snapshot.starts_with("big.txt\n"), "{snapshot}");
    assert!(snapshot.contains("1:line-1"), "{snapshot}");
    assert!(snapshot.contains("80:line-80"), "{snapshot}");
    assert!(snapshot.contains("…\n"), "{snapshot}");
    assert!(
        snapshot.contains(&chain_truncation_footer(36, 80)),
        "{snapshot}"
    );
    assert!(!snapshot.contains("40:line-40"), "{snapshot}");
}

// Covers: failure focus keeps anchors without dumping the whole file
// Owner: text view
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
