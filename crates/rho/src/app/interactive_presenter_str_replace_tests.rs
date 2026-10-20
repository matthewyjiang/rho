use pretty_assertions::assert_eq;
use rho_tools::tool_card::{DiffRow, DiffRowKind, ToolBody, ToolFact, ToolStatus};

use super::str_replace_card;

fn row(kind: DiffRowKind, line: Option<u32>, text: &str) -> DiffRow {
    DiffRow::new(kind, line, text)
}

// Covers: proposed str_replace cards show the change before the tool runs: the
// exact file diff when the live file has an applicable match, otherwise a
// numberless projection with up to three shared edge lines; a changed final
// newline still shows; a no-op stays silent.
// Owner: interactive presenter str_replace card
#[test]
fn proposed_str_replace_card_projects_old_and_new_strings() {
    use DiffRowKind::{Added, Context, Removed, Skip};

    let insert = serde_json::json!({
        "path": "src/lib.rs",
        "old_string": "a\nb",
        "new_string": "a\nX\nb",
    });
    let cases = [
        (
            "unique live match renders the exact file diff",
            insert.clone(),
            Some("z\na\nb\n"),
            ToolBody::Diff(vec![
                row(Context, Some(1), "z"),
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
                row(Context, Some(1), "z"),
                row(Context, Some(2), "a"),
                row(Added, Some(3), "X"),
                row(Context, Some(4), "b"),
            ]),
        ),
        (
            "bare cr live file still matches",
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "a\nb",
                "new_string": "a\nX\nb",
            }),
            Some("a\rb"),
            ToolBody::Diff(vec![
                row(Context, Some(1), "a"),
                row(Added, Some(2), "X"),
                row(Context, Some(3), "b"),
            ]),
        ),
        (
            "long shared context without live file trims to three lines",
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "c1\nc2\nc3\nc4\nold\nt1\nt2\nt3\nt4",
                "new_string": "c1\nc2\nc3\nc4\nnew\nt1\nt2\nt3\nt4",
            }),
            None,
            ToolBody::Diff(vec![
                row(Skip, None, "⋯"),
                row(Context, None, "c2"),
                row(Context, None, "c3"),
                row(Context, None, "c4"),
                row(Removed, None, "old"),
                row(Added, None, "new"),
                row(Context, None, "t1"),
                row(Context, None, "t2"),
                row(Context, None, "t3"),
                row(Skip, None, "⋯"),
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
            ToolBody::Diff(vec![row(Removed, None, "foo"), row(Added, None, "foo")]),
        ),
        (
            "deleted terminated line",
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "foo\n",
                "new_string": "",
            }),
            None,
            ToolBody::Diff(vec![row(Removed, None, "foo")]),
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

// Covers: a replace_all approval card states how many occurrences it edits and
// scales its line stats, or omits them when the count is unknown, so it never
// understates the approved change.
// Owner: interactive presenter str_replace card
#[test]
fn replace_all_card_reports_every_occurrence() {
    let arguments = serde_json::json!({
        "path": "src/lib.rs",
        "old_string": "foo",
        "new_string": "bar",
        "replace_all": true,
    });
    let stat = |n| ToolFact::DiffStat {
        added: n,
        removed: n,
        path: None,
    };
    let meta = |text: &str| ToolFact::Meta { text: text.into() };
    let cases = [
        (
            "three live matches",
            Some("foo\nx\nfoo\nfoo\n"),
            vec![stat(3), meta("replaces all 3 occurrences")],
        ),
        (
            "two matches on one line",
            Some("foo foo\nx\n"),
            vec![stat(1), meta("replaces all 2 occurrences")],
        ),
        ("one live match", Some("foo\nx\n"), vec![stat(1)]),
        (
            "unknown file",
            None,
            vec![meta("replaces every occurrence")],
        ),
    ];

    for (name, live_file, expected) in cases {
        let card = str_replace_card(
            &arguments,
            std::path::Path::new("/workspace"),
            ToolStatus::Running,
            live_file,
        );
        assert_eq!(card.facts, expected, "{name}");
    }
}
