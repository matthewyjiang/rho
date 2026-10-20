use pretty_assertions::assert_eq;
use rho_tools::tool_card::{DiffRow, DiffRowKind, ToolBody, ToolStatus};

use super::str_replace_card;

fn row(kind: DiffRowKind, line: Option<u32>, text: &str) -> DiffRow {
    DiffRow::new(kind, line, text)
}

// Covers: proposed str_replace cards show the change before the tool runs,
// with shared edge lines as context and live line numbers only when the
// match is unique; a changed final newline still shows; a no-op stays silent.
// Owner: interactive presenter str_replace card
#[test]
fn proposed_str_replace_card_projects_old_and_new_strings() {
    use DiffRowKind::{Added, Context, Removed};

    let insert = serde_json::json!({
        "path": "src/lib.rs",
        "old_string": "a\nb",
        "new_string": "a\nX\nb",
    });
    let cases = [
        (
            "unique live match numbers rows",
            insert.clone(),
            Some("z\na\nb\n"),
            ToolBody::Diff(vec![
                row(Context, Some(2), "a"),
                row(Added, Some(3), "X"),
                row(Context, Some(4), "b"),
            ]),
        ),
        (
            "ambiguous live match drops numbers",
            insert.clone(),
            Some("a\nb\na\nb\n"),
            ToolBody::Diff(vec![
                row(Context, None, "a"),
                row(Added, None, "X"),
                row(Context, None, "b"),
            ]),
        ),
        (
            "crlf live file still matches",
            insert,
            Some("z\r\na\r\nb\r\n"),
            ToolBody::Diff(vec![
                row(Context, Some(2), "a"),
                row(Added, Some(3), "X"),
                row(Context, Some(4), "b"),
            ]),
        ),
        (
            "replacement without live file",
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "foo",
                "new_string": "bar",
            }),
            None,
            ToolBody::Diff(vec![row(Removed, None, "foo"), row(Added, None, "bar")]),
        ),
        (
            "partial stream before new_string",
            serde_json::json!({ "path": "src/lib.rs", "old_string": "foo" }),
            None,
            ToolBody::Diff(vec![row(Removed, None, "foo")]),
        ),
        (
            "removed trailing newline",
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "foo\n",
                "new_string": "foo",
            }),
            None,
            ToolBody::Diff(vec![row(Context, None, "foo"), row(Removed, None, "")]),
        ),
        (
            "shared trailing newline",
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "foo\n",
                "new_string": "bar\n",
            }),
            None,
            ToolBody::Diff(vec![row(Removed, None, "foo"), row(Added, None, "bar")]),
        ),
        (
            "no-op replacement",
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "same",
                "new_string": "same",
            }),
            None,
            ToolBody::None,
        ),
    ];

    for (name, arguments, live_file, expected) in cases {
        let card = str_replace_card(
            &arguments,
            std::path::Path::new("/workspace"),
            ToolStatus::Running,
            live_file,
        );
        assert_eq!(card.body, expected, "{name}");
    }
}
