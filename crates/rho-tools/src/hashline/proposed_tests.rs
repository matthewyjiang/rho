use pretty_assertions::assert_eq;

use super::*;

// Covers: proposal cards must show op summaries and PUT bodies from the document
// alone so approval does not need the target file
// Owner: hashline proposed
#[test]
fn projects_mixed_ops_into_document_rows() {
    let proposed = proposed_edit(
        r#"[src/a.rs#A1B2]
PUT 2.=3:
+TWO
+THREE
CUT 5.=5
PUT >6:
+tail
"#,
    );
    assert_eq!(proposed.files.len(), 1);
    let file = &proposed.files[0];
    assert_eq!(file.path, "src/a.rs");
    assert_eq!(file.added_lines, 3);
    assert_eq!(file.removed_lines, 3);
    assert_eq!(
        file.rows,
        vec![
            ProposedRow::Summary("PUT 2.=3".into()),
            ProposedRow::Added("TWO".into()),
            ProposedRow::Added("THREE".into()),
            ProposedRow::Summary("CUT 5".into()),
            ProposedRow::Summary("PUT >6".into()),
            ProposedRow::Added("tail".into()),
        ]
    );
    assert!(!proposed.truncated);
}

// Covers: incomplete streamed documents still yield path counts and partial rows
// Owner: hashline proposed
#[test]
fn projects_incomplete_documents() {
    let proposed =
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
    assert_eq!(proposed.files[0].rows.len(), 4);
    assert_eq!(proposed.files[1].rows.len(), 2);
    assert_eq!(
        proposed.files[0].rows[0],
        ProposedRow::Summary("PUT 1.=3".into())
    );
    assert_eq!(
        proposed.files[0].rows[3],
        ProposedRow::Summary("CUT 8.=9".into())
    );
}
