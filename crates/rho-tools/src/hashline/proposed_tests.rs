use pretty_assertions::assert_eq;

use super::*;
use crate::{
    hashline::format::compute_file_hash,
    tool_card::{DiffCardChange, DiffRowKind},
};

// Covers: proposal cards must show op summaries and PUT bodies from the document
// alone so streaming does not need the target file
// Owner: hashline proposed
#[test]
fn projects_mixed_ops_into_document_rows() {
    let preview = proposed_edit(
        r#"[src/a.rs#A1B2]
PUT 2.=3:
+TWO
+THREE
CUT 5.=5
PUT >6:
+tail
"#,
    );
    assert_eq!(preview.files.len(), 1);
    let file = &preview.files[0];
    assert_eq!(file.path, "src/a.rs");
    assert_eq!(file.change, DiffCardChange::Content);
    assert_eq!(file.stats, Some((3, 3)));
    assert_eq!(
        file.rows
            .iter()
            .map(|row| (row.kind, row.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (DiffRowKind::Meta, "PUT 2.=3"),
            (DiffRowKind::Added, "TWO"),
            (DiffRowKind::Added, "THREE"),
            (DiffRowKind::Meta, "CUT 5"),
            (DiffRowKind::Meta, "PUT >6"),
            (DiffRowKind::Added, "tail"),
        ]
    );
    assert!(!preview.truncated);
    assert_eq!(preview.kind, EditPreviewKind::Document);
    assert!(!preview.warns_unverified());
}

// Covers: incomplete streamed documents still yield path counts and partial rows
// Owner: hashline proposed
#[test]
fn projects_incomplete_documents() {
    let preview =
        proposed_edit("[a.rs#ABCD]\nPUT 1.=3:\n+one\n+two\nCUT 8.=9\n[b.rs#ABCD]\nPUT 2:\n+pa");
    assert_eq!(
        proposed_sections("[a.rs#ABCD]\nPUT 1.=3:\n+one\n+two\nCUT 8.=9\n[b.rs#ABCD]\nPUT 2:\n+pa"),
        vec![
            ProposedSection {
                path: "a.rs".into(),
                added_lines: 2,
                removed_lines: 5,
            },
            ProposedSection {
                path: "b.rs".into(),
                added_lines: 1,
                removed_lines: 1,
            },
        ]
    );
    // PUT summary + 2 bodies + CUT summary; PUT summary + body (partial stream)
    assert_eq!(preview.files[0].rows.len(), 4);
    assert_eq!(preview.files[1].rows.len(), 2);
    assert_eq!(preview.files[0].rows[0].text, "PUT 1.=3");
    assert_eq!(preview.files[0].rows[0].kind, DiffRowKind::Meta);
    assert_eq!(preview.files[0].rows[3].text, "CUT 8.=9");
    assert_eq!(preview.files[0].rows[3].kind, DiffRowKind::Meta);
}

// Covers: pure CUT sections stay Content with removal stats (not file-delete)
// Owner: hashline proposed
#[test]
fn pure_cut_stays_content_change_with_stats() {
    let preview = proposed_edit("[gone.txt#AAAA]\nCUT 1.=3\n");
    let file = &preview.files[0];
    assert_eq!(file.change, DiffCardChange::Content);
    assert_eq!(file.stats, Some((0, 3)));
    assert!(file
        .rows
        .iter()
        .any(|row| row.kind == DiffRowKind::Meta && row.text == "CUT 1.=3"));
}

// Covers: planned_edit applies against live text and surfaces removed lines
// Owner: hashline proposed
#[test]
fn plans_live_content_diff_with_removals() {
    let original = "alpha\nbeta\ngamma\n";
    let tag = compute_file_hash(original);
    let input = format!("[sample.txt#{tag}]\nPUT 2.=2:\n+BETA\nCUT 3.=3\n");
    let planned = planned_edit(&input, |path| {
        assert_eq!(path, "sample.txt");
        Some(original.to_string())
    });
    assert_eq!(planned.files.len(), 1);
    let file = &planned.files[0];
    assert_eq!(planned.kind, EditPreviewKind::Planned { unverified: false });
    assert!(!planned.warns_unverified());
    assert_eq!(file.change, DiffCardChange::Content);
    assert!(
        file.rows
            .iter()
            .any(|row| row.kind == DiffRowKind::Removed && row.text == "beta"),
        "expected removed beta: {:?}",
        file.rows
    );
    assert!(
        file.rows
            .iter()
            .any(|row| row.kind == DiffRowKind::Removed && row.text == "gamma"),
        "expected removed gamma: {:?}",
        file.rows
    );
    assert!(
        file.rows
            .iter()
            .any(|row| row.kind == DiffRowKind::Added && row.text == "BETA"),
        "expected added BETA: {:?}",
        file.rows
    );
    let (added, removed) = file.stats.expect("stats");
    assert!(removed >= 2);
    assert!(added >= 1);
}

// Covers: planned_edit falls back to document rows with an explicit notice when
// the live file is missing
// Owner: hashline proposed
#[test]
fn planned_edit_falls_back_without_live_file() {
    let planned = planned_edit("[missing.txt#AAAA]\nPUT 1.=1:\n+x\n", |_| None);
    assert!(planned.warns_unverified());
    assert_eq!(planned.kind, EditPreviewKind::Planned { unverified: true });
    assert!(planned.files.iter().all(|file| {
        file.rows.first().is_some_and(|row| {
            row.kind == DiffRowKind::Meta && row.text == EDIT_DOCUMENT_ONLY_NOTICE
        })
    }));
    assert!(planned
        .files
        .iter()
        .flat_map(|f| &f.rows)
        .all(|row| row.kind != DiffRowKind::Removed));
}
