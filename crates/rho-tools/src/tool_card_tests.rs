use pretty_assertions::assert_eq;

use super::*;

fn base_card() -> ToolCard {
    ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::call("tool", Some("x".into())),
    )
}

fn meta_facts(count: usize) -> Vec<ToolFact> {
    (0..count)
        .map(|i| ToolFact::Meta {
            text: format!("fact-{i}"),
        })
        .collect()
}

fn diff_body() -> ToolBody {
    ToolBody::Diff(vec![
        DiffRow::new(DiffRowKind::Removed, Some(41), "old"),
        DiffRow::new(DiffRowKind::Added, Some(41), "new"),
    ])
}

// Covers: collapsed/expanded display budgets share one row budget across facts
// and bodies, and diffs collapse like any other body
// Owner: pure unit (tool card layout)
#[test]
fn display_plan_shares_one_row_budget() {
    let lines = |count: usize| ToolBody::Lines((0..count).map(|i| format!("line-{i}")).collect());
    for (case, card, budget, expanded, expected) in [
        (
            "short diff collapsed shows body",
            base_card().with_body(diff_body()),
            10,
            false,
            ToolCardDisplayPlan {
                visible_facts: 0,
                visible_body_lines: 2,
                hidden_rows: 0,
                expandable: false,
                show_collapse_prompt: false,
            },
        ),
        (
            "diff past budget collapses like any body",
            base_card().with_body(diff_body()),
            1,
            false,
            ToolCardDisplayPlan {
                visible_facts: 0,
                visible_body_lines: 1,
                hidden_rows: 1,
                expandable: true,
                show_collapse_prompt: false,
            },
        ),
        (
            "expanded diff past budget shows collapse prompt",
            base_card().with_body(diff_body()),
            1,
            true,
            ToolCardDisplayPlan {
                visible_facts: 0,
                visible_body_lines: 2,
                hidden_rows: 0,
                expandable: true,
                show_collapse_prompt: true,
            },
        ),
        (
            "long body truncated when collapsed",
            base_card().with_body(lines(8)),
            3,
            false,
            ToolCardDisplayPlan {
                visible_facts: 0,
                visible_body_lines: 3,
                hidden_rows: 5,
                expandable: true,
                show_collapse_prompt: false,
            },
        ),
        (
            "many facts exceed budget when collapsed",
            base_card().with_facts(meta_facts(5)),
            2,
            false,
            ToolCardDisplayPlan {
                visible_facts: 2,
                visible_body_lines: 0,
                hidden_rows: 3,
                expandable: true,
                show_collapse_prompt: false,
            },
        ),
        (
            "facts and body share one budget",
            base_card().with_facts(meta_facts(2)).with_body(lines(5)),
            3,
            false,
            ToolCardDisplayPlan {
                visible_facts: 2,
                visible_body_lines: 1,
                hidden_rows: 4,
                expandable: true,
                show_collapse_prompt: false,
            },
        ),
        (
            "expanded long body shows collapse prompt",
            base_card().with_facts(meta_facts(1)).with_body(lines(8)),
            3,
            true,
            ToolCardDisplayPlan {
                visible_facts: 1,
                visible_body_lines: 8,
                hidden_rows: 0,
                expandable: true,
                show_collapse_prompt: true,
            },
        ),
    ] {
        assert_eq!(card.display_plan(budget, expanded), expected, "{case}");
    }
}

// Covers: multi-file unified diffs must not invent blank separators as content
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

#[test]
fn diff_stats_multi_file_without_blank_separator() {
    let diff = "\
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-a
+A
--- a/b.rs
+++ b/b.rs
@@ -1 +1 @@
-b
+B
";
    assert_eq!(
        diff_file_stats(diff),
        vec![
            DiffFileStat {
                path: "a.rs".into(),
                added: 1,
                removed: 1,
            },
            DiffFileStat {
                path: "b.rs".into(),
                added: 1,
                removed: 1,
            },
        ]
    );
}

// Covers: ToolFact serde roundtrip keeps the timeout budget structured so
// hosts can decorate it live without parsing display text.
// Owner: pure unit (tool card)
#[test]
fn tool_fact_timeout_serde_roundtrip() {
    let cases = [
        (
            ToolFact::Timeout { seconds: Some(30) },
            r#"{"kind":"timeout","seconds":30}"#,
        ),
        (ToolFact::Timeout { seconds: None }, r#"{"kind":"timeout"}"#),
    ];
    for (fact, expected_json) in cases {
        let json = serde_json::to_string(&fact).unwrap();
        assert_eq!(json, expected_json);
        let decoded: ToolFact = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, fact);
    }
}
