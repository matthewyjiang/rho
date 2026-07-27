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

#[test]
fn display_plan_short_diff_collapsed_shows_body() {
    let card = base_card().with_body(diff_body());
    let plan = card.display_plan(10, /*expanded*/ false);
    assert_eq!(
        plan,
        ToolCardDisplayPlan {
            visible_facts: 0,
            visible_body_lines: 2,
            hidden_rows: 0,
            expandable: false,
            show_collapse_prompt: false,
        }
    );
}

#[test]
fn display_plan_diff_past_budget_collapses_like_any_body() {
    let card = base_card().with_body(diff_body());
    let plan = card.display_plan(1, /*expanded*/ false);
    assert_eq!(
        plan,
        ToolCardDisplayPlan {
            visible_facts: 0,
            visible_body_lines: 1,
            hidden_rows: 1,
            expandable: true,
            show_collapse_prompt: false,
        }
    );
}

#[test]
fn display_plan_expanded_diff_shows_collapse_prompt_past_budget() {
    let card = base_card().with_body(diff_body());
    let plan = card.display_plan(1, /*expanded*/ true);
    assert_eq!(
        plan,
        ToolCardDisplayPlan {
            visible_facts: 0,
            visible_body_lines: 2,
            hidden_rows: 0,
            expandable: true,
            show_collapse_prompt: true,
        }
    );
}

#[test]
fn display_plan_long_non_diff_body_truncated_when_collapsed() {
    let body = (0..8).map(|i| format!("line-{i}")).collect();
    let card = base_card().with_body(ToolBody::Lines(body));
    let plan = card.display_plan(3, /*expanded*/ false);
    assert_eq!(
        plan,
        ToolCardDisplayPlan {
            visible_facts: 0,
            visible_body_lines: 3,
            hidden_rows: 5,
            expandable: true,
            show_collapse_prompt: false,
        }
    );
}

#[test]
fn display_plan_many_facts_exceed_budget_when_collapsed() {
    let card = base_card().with_facts(meta_facts(5));
    let plan = card.display_plan(2, /*expanded*/ false);
    assert_eq!(
        plan,
        ToolCardDisplayPlan {
            visible_facts: 2,
            visible_body_lines: 0,
            hidden_rows: 3,
            expandable: true,
            show_collapse_prompt: false,
        }
    );
}

#[test]
fn display_plan_facts_and_body_share_one_budget() {
    let body = (0..5).map(|i| format!("line-{i}")).collect();
    let card = base_card()
        .with_facts(meta_facts(2))
        .with_body(ToolBody::Lines(body));
    let plan = card.display_plan(3, /*expanded*/ false);
    assert_eq!(
        plan,
        ToolCardDisplayPlan {
            visible_facts: 2,
            visible_body_lines: 1,
            hidden_rows: 4,
            expandable: true,
            show_collapse_prompt: false,
        }
    );
}

#[test]
fn display_plan_expanded_long_body_shows_collapse_prompt() {
    let body = (0..8).map(|i| format!("line-{i}")).collect();
    let card = base_card()
        .with_facts(meta_facts(1))
        .with_body(ToolBody::Lines(body));
    let plan = card.display_plan(3, /*expanded*/ true);
    assert_eq!(
        plan,
        ToolCardDisplayPlan {
            visible_facts: 1,
            visible_body_lines: 8,
            hidden_rows: 0,
            expandable: true,
            show_collapse_prompt: true,
        }
    );
}

#[test]
fn compact_diff_rows_two_files_without_blank_separator() {
    // No blank line between file sections - second header must not become +/- content.
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
fn diff_rows_render_as_signed_plain_text() {
    let rows = vec![
        DiffRow::new(DiffRowKind::File, None, "src/lib.rs"),
        DiffRow::new(DiffRowKind::Context, Some(1), "keep"),
        DiffRow::new(DiffRowKind::Added, Some(2), "new"),
        DiffRow::new(DiffRowKind::Removed, Some(2), "old"),
        DiffRow::new(DiffRowKind::Added, None, "unnumbered"),
    ];

    assert_eq!(
        ToolBody::Diff(rows).plain_lines(),
        vec![
            "src/lib.rs".to_string(),
            "1  keep".to_string(),
            "2 +new".to_string(),
            "2 -old".to_string(),
            "+unnumbered".to_string(),
        ]
    );
}

#[test]
fn diff_stats_count_per_file() {
    let diff = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new

--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-before
+after
";
    assert_eq!(
        diff_file_stats(diff),
        vec![
            DiffFileStat {
                path: "src/lib.rs".into(),
                added: 1,
                removed: 1,
            },
            DiffFileStat {
                path: "src/main.rs".into(),
                added: 1,
                removed: 1,
            },
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

#[test]
fn created_file_uses_new_path() {
    let diff = "\
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+first
+second
";
    assert_eq!(
        parse_unified_diff(diff)
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>(),
        vec!["src/new.rs".to_string()]
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

#[test]
fn card_header_and_facts_include_marker_and_diff_stat() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::call("edit_file", Some("theme.rs".into())),
    )
    .with_facts(vec![ToolFact::DiffStat {
        added: 54,
        removed: 2,
        path: Some("theme.rs".into()),
    }]);
    assert_eq!(card.header_text(), "✓ edit_file(theme.rs)");
    assert_eq!(
        card.facts,
        vec![ToolFact::DiffStat {
            added: 54,
            removed: 2,
            path: Some("theme.rs".into()),
        }]
    );
    assert_eq!(card.facts[0].plain_text(), "+54 -2 lines | theme.rs");
}

#[test]
fn tool_body_variants_round_trip() {
    for body in [
        ToolBody::None,
        ToolBody::Lines(vec!["line".into()]),
        ToolBody::Diff(vec![DiffRow::new(DiffRowKind::Added, Some(1), "line")]),
    ] {
        let encoded = serde_json::to_string(&body).unwrap();
        assert_eq!(serde_json::from_str::<ToolBody>(&encoded).unwrap(), body);
    }
}

#[test]
fn card_round_trips_through_json() {
    let card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Web,
        ToolHeader::call("web_search", Some("\"rust\"".into())),
    )
    .with_facts(vec![ToolFact::Count {
        label: "results".into(),
        value: 8,
        detail: Some("stored".into()),
    }])
    .with_body(ToolBody::Lines(vec!["body".into()]));
    let encoded = serde_json::to_string(&card).unwrap();
    let decoded: ToolCard = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, card);
}
