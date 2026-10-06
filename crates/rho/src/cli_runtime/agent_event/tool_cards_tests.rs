use agent_client_protocol::schema::v1::{Terminal, ToolCallUpdateFields};
use pretty_assertions::assert_eq;

use super::*;

// Covers: partial ACP snapshots must retain omitted fields, but replace content
// and locations when explicitly supplied (including empty collections).
// Owner: pure ACP card construction; renderer tests cover lifecycle pairing.
#[test]
fn partial_updates_preserve_and_replace_card_fields() {
    let cwd = Path::new("/workspace");
    let call = ToolCall::new("tool", "Inspect config")
        .kind(ToolKind::Read)
        .locations(vec![ToolCallLocation::new("/workspace/src/config.rs")])
        .content(vec!["before".into()]);
    let (mut tool, notices) = started_card(&call, cwd);
    let initial = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileCommand,
        ToolHeader::call("Read", Some("src/config.rs".into())),
    )
    .with_body(ToolBody::Lines(vec!["before".into()]));
    assert_eq!((tool.card.clone(), notices), (initial.clone(), vec![]));

    let replacement = initial
        .clone()
        .with_facts(vec![ToolFact::Meta {
            text: "terminal: tty-1".into(),
        }])
        .with_body(ToolBody::Lines(vec!["after".into()]));
    let empty = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileCommand,
        ToolHeader::call("Read", Some("src/config.rs".into())),
    );
    let cases = vec![
        (
            ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
            initial,
        ),
        (
            ToolCallUpdateFields::new().content(vec![
                "after".into(),
                ToolCallContent::Terminal(Terminal::new("tty-1")),
            ]),
            replacement,
        ),
        (ToolCallUpdateFields::new().content(vec![]), empty),
        (
            ToolCallUpdateFields::new()
                .locations(vec![])
                .status(ToolCallStatus::Completed),
            ToolCard::new(
                ToolStatus::Ok,
                ToolFamily::FileCommand,
                ToolHeader::call("Read", Some("Inspect config".into())),
            ),
        ),
    ];
    for (fields, expected) in cases {
        assert_eq!(
            finished_card(&mut tool, &ToolCallUpdate::new("tool", fields), cwd),
            (expected, vec![])
        );
    }
}

// Covers: diff stats count actual changed lines, not unchanged file context or
// truncated display rows; mixed text content is not lost behind the diff body.
// Owner: pure ACP card construction.
#[test]
fn diffs_preserve_change_counts_and_bound_the_body() {
    struct Case {
        name: &'static str,
        content: Vec<ToolCallContent>,
        expected: ToolCard,
    }
    let base = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileDiff,
        ToolHeader::call("Edit", Some("Edit config".into())),
    );
    let cases = vec![
        Case {
            name: "unchanged context is not a removal/addition",
            content: vec![
                Diff::new("/workspace/config", "keep\nnew\n")
                    .old_text("keep\nold\n")
                    .into(),
                "updated".into(),
            ],
            expected: base
                .clone()
                .with_facts(vec![ToolFact::DiffStat {
                    added: 1,
                    removed: 1,
                    path: Some("config".into()),
                }])
                .with_body(ToolBody::Diff(vec![
                    DiffRow::new(DiffRowKind::Context, Some(1), "keep"),
                    DiffRow::new(DiffRowKind::Removed, Some(2), "old"),
                    DiffRow::new(DiffRowKind::Added, Some(2), "new"),
                    DiffRow::new(DiffRowKind::Meta, /*line*/ None, "updated"),
                ])),
        },
        Case {
            name: "new file stats survive line truncation",
            content: vec![Diff::new(
                "/workspace/config",
                "line\n".repeat(MAX_TOOL_BODY_LINES + 1),
            )
            .into()],
            expected: base
                .with_facts(vec![ToolFact::DiffStat {
                    added: (MAX_TOOL_BODY_LINES + 1) as u64,
                    removed: 0,
                    path: Some("config".into()),
                }])
                .with_body(ToolBody::Diff({
                    let mut rows = (1..=MAX_TOOL_BODY_LINES)
                        .map(|line| DiffRow::new(DiffRowKind::Added, Some(line as u32), "line"))
                        .collect::<Vec<_>>();
                    rows.push(DiffRow::new(
                        DiffRowKind::Skip,
                        /*line*/ None,
                        "… [truncated diff]",
                    ));
                    rows
                })),
        },
    ];
    for case in cases {
        let call = ToolCall::new("edit", "Edit config")
            .kind(ToolKind::Edit)
            .status(ToolCallStatus::Completed)
            .content(case.content);
        let (tool, notices) = started_card(&call, Path::new("/workspace"));
        assert_eq!(
            (tool.card, notices),
            (case.expected, vec![]),
            "{}",
            case.name
        );
    }
}

// Covers: a wide Unicode tool output cannot overflow retained card payloads or
// error summaries. These payload limits are independent of diff row limits.
// Owner: pure ACP card construction.
#[test]
fn wide_error_payload_is_bounded_without_splitting_unicode() {
    let call = ToolCall::new("tool", "Inspect")
        .status(ToolCallStatus::Failed)
        .content(vec!["界".repeat(MAX_TOOL_PAYLOAD_CHARS + 1).into()]);
    let (tool, notices) = started_card(&call, Path::new("/workspace"));
    let expected = ToolCard::new(
        ToolStatus::Error,
        ToolFamily::Default,
        ToolHeader::call("Inspect", /*primary*/ None),
    )
    .with_facts(vec![ToolFact::Error {
        text: format!("{}…", "界".repeat(ERROR_SUMMARY_CHARS - 1)),
    }])
    .with_body(ToolBody::Lines(vec![format!(
        "{}… [truncated tool payload]",
        "界".repeat(MAX_TOOL_PAYLOAD_CHARS),
    )]));
    assert_eq!((tool.card, notices), (expected, vec![]));
}
