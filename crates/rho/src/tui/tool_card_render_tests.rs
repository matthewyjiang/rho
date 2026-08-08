use pretty_assertions::assert_eq;
use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus,
};

use super::push_tool_card;

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn render(card: &ToolCard, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    push_tool_card(
        &mut lines, card, width, /*max_tool_output_lines*/ 32, /*expanded*/ true,
    );
    lines.into_iter().map(|line| line_text(&line)).collect()
}

// Covers: wrapped fact rows keep a tree stem so long child text stays tied to the trunk.
// Owner: pure TUI layout
#[test]
fn mid_fact_wrap_keeps_box_stem_before_later_sibling() {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Web,
        ToolHeader::call("x_search", None),
    );
    card.push_fact(ToolFact::Text {
        text: "Cerebras (5.6 OR o3) coming or release soon".into(),
    });
    card.push_fact(ToolFact::Meta {
        text: "finished".into(),
    });

    let lines = render(&card, 28);
    assert_eq!(lines[0], "✓ x_search");
    assert!(
        lines[1].starts_with("  ├ "),
        "first fact row should branch: {:?}",
        lines
    );
    assert!(
        lines.iter().any(|line| line.starts_with("  │ ")),
        "wrapped mid fact should extend with │: {:?}",
        lines
    );
    assert_eq!(lines.last().map(String::as_str), Some("  └ finished"));
}

// Covers: last fact wrap must not leave a dangling │ under └.
// Owner: pure TUI layout
#[test]
fn last_fact_wrap_uses_space_hang_not_stem() {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Web,
        ToolHeader::call("x_search", None),
    );
    card.push_fact(ToolFact::Text {
        text: "only child query that needs several wraps at this width".into(),
    });

    let lines = render(&card, 24);
    assert!(
        lines[1].starts_with("  └ "),
        "sole fact should be last branch: {:?}",
        lines
    );
    let continuations: Vec<_> = lines.iter().skip(2).collect();
    assert!(
        !continuations.is_empty(),
        "expected wrap continuations: {:?}",
        lines
    );
    for line in continuations {
        assert!(
            line.starts_with("    ") && !line.starts_with("  │ "),
            "last-child wrap must hang with spaces: {:?}",
            lines
        );
    }
}

// Covers: multi-file File headers keep a continuous trunk through body rows so
// section branches read as one tree (│ under mid files, hang under the last).
// Owner: pure TUI layout
#[test]
fn multi_file_diff_connects_body_under_section_headers() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileDiff,
        ToolHeader::call("edit", Some("2 files".into())),
    )
    .with_body(ToolBody::Diff(vec![
        DiffRow::file_header("a.txt", Some((1, 1))),
        DiffRow::new(DiffRowKind::Added, Some(1), "A"),
        DiffRow::file_header("b.txt", Some((0, 1))),
        DiffRow::new(DiffRowKind::Removed, Some(1), "B"),
    ]));

    let lines = render(&card, 40);
    assert_eq!(lines[0], "✓ edit(2 files)");
    assert!(
        lines[1].starts_with("  ├ ")
            && lines[1].contains("+1 -1 lines")
            && lines[1].contains("a.txt"),
        "first file section should mid-branch: {:?}",
        lines
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("  │ ") && line.contains("A")),
        "body under first file must keep trunk stem: {:?}",
        lines
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("  └ ") && line.contains("b.txt")),
        "last file section should end-branch: {:?}",
        lines
    );
    assert!(
        lines.iter().any(|line| {
            line.contains("B") && line.starts_with("    ") && !line.starts_with("  │ ")
        }),
        "body under last file must hang without stem: {:?}",
        lines
    );
}

// Covers: fact wrap prefers whitespace over hard mid-word cuts (same as headers).
// Owner: pure TUI layout
#[test]
fn fact_wrap_breaks_on_whitespace() {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("tool", None),
    );
    card.push_fact(ToolFact::Text {
        text: "one two three four".into(),
    });

    // prefix "  └ " is 4 cols; content width 10.
    // Soft wrap: "one two" / "three four". Hard wrap would cut "three".
    let lines = render(&card, 14);
    let joined = lines.join("\n");
    assert!(
        !joined.contains("one two th"),
        "must not hard-split inside 'three': {:?}",
        lines
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("three four") || line.ends_with("three")),
        "expected whitespace-bounded wrap rows: {:?}",
        lines
    );
}
