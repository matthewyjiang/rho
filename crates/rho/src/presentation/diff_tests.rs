use pretty_assertions::assert_eq;

use super::*;

// Covers: multi-file unified diffs must not invent blank separators as
// content, and count each file's own additions and removals.
// Owner: pure unit (diff parser)
#[test]
fn compact_diff_rows_two_files_without_blank_separator() {
    let diff = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
--- a/src/main.rs
+++ b/src/main.rs
@@ -12 +12 @@
-before
+after
";
    let stats: Vec<(String, u64, u64)> =
        vec![("src/lib.rs".into(), 1, 1), ("src/main.rs".into(), 1, 1)];
    assert_eq!(
        parse_unified_diff(diff)
            .into_iter()
            .map(|file| (file.path, file.added, file.removed))
            .collect::<Vec<_>>(),
        stats
    );
    assert_eq!(
        compact_diff_rows(diff, /*include_file_headers*/ true),
        vec![
            DiffRow::new(DiffRowKind::File, None, "src/lib.rs"),
            DiffRow::new(DiffRowKind::Removed, Some(1), "old"),
            DiffRow::new(DiffRowKind::Added, Some(1), "new"),
            DiffRow::new(DiffRowKind::File, None, "src/main.rs"),
            DiffRow::new(DiffRowKind::Removed, Some(12), "before"),
            DiffRow::new(DiffRowKind::Added, Some(12), "after"),
        ]
    );
    assert_eq!(
        compact_diff_rows(diff, /*include_file_headers*/ false),
        vec![
            DiffRow::new(DiffRowKind::Removed, Some(1), "old"),
            DiffRow::new(DiffRowKind::Added, Some(1), "new"),
            DiffRow::new(DiffRowKind::Removed, Some(12), "before"),
            DiffRow::new(DiffRowKind::Added, Some(12), "after"),
        ]
    );
}

#[test]
fn compact_diff_rows_number_context_and_mark_hunk_gaps() {
    let diff = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -40,3 +40,3 @@
 keep
-old
+new
@@ -80,2 +80,3 @@
 tail
+added
";
    assert_eq!(
        compact_diff_rows(diff, /*include_file_headers*/ false),
        vec![
            DiffRow::new(DiffRowKind::Context, Some(40), "keep"),
            DiffRow::new(DiffRowKind::Removed, Some(41), "old"),
            DiffRow::new(DiffRowKind::Added, Some(41), "new"),
            DiffRow::new(DiffRowKind::Skip, None, "⋯"),
            DiffRow::new(DiffRowKind::Context, Some(80), "tail"),
            DiffRow::new(DiffRowKind::Added, Some(81), "added"),
        ]
    );
}

#[test]
fn deleted_file_keeps_old_path_not_dev_null() {
    let diff = "\
--- a/src/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-first
-second
";
    let files = parse_unified_diff(diff);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/gone.rs");
    assert_eq!(files[0].added, 0);
    assert_eq!(files[0].removed, 2);
    assert_eq!(
        compact_diff_rows_from_files(&files, /*include_file_headers*/ true),
        vec![
            DiffRow::new(DiffRowKind::File, None, "src/gone.rs"),
            DiffRow::new(DiffRowKind::Removed, Some(1), "first"),
            DiffRow::new(DiffRowKind::Removed, Some(2), "second"),
        ]
    );
}

// Covers: multi-file card sections keep path + stats on File rows so the
// presenter can drop DiffStat facts without losing identity or counts.
// Owner: pure unit (tool card)
#[test]
fn multi_file_card_headers_include_content_stats() {
    let files = vec![
        DiffCardFile {
            path: "a.rs".into(),
            source_path: None,
            change: DiffCardChange::Content,
            stats: Some((1, 1)),
            rows: vec![DiffRow::new(DiffRowKind::Added, Some(1), "A")],
        },
        DiffCardFile {
            path: "b.rs".into(),
            source_path: None,
            change: DiffCardChange::Content,
            stats: Some((0, 2)),
            rows: vec![DiffRow::new(DiffRowKind::Removed, Some(1), "x")],
        },
    ];
    assert_eq!(
        compact_diff_rows_from_card_files(&files, /*include_file_headers*/ true),
        vec![
            DiffRow::file_header("a.rs", Some((1, 1))),
            DiffRow::new(DiffRowKind::Added, Some(1), "A"),
            DiffRow::file_header("b.rs", Some((0, 2))),
            DiffRow::new(DiffRowKind::Removed, Some(1), "x"),
        ]
    );
    assert_eq!(
        DiffRow::file_header("a.rs", Some((1, 1))).plain_text(),
        "+1 -1 lines | a.rs"
    );
}
