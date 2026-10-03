use pretty_assertions::assert_eq;

use super::*;
use crate::tui::{ReasoningEntry, ToolEntry};

fn tool() -> Entry {
    Entry::Tool(ToolEntry::new(
        rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Ok,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("read_file(a.rs)", None),
        ),
        false,
        None,
        None,
    ))
}

fn hides_work(entry: &Entry) -> bool {
    matches!(entry, Entry::Tool(_) | Entry::Reasoning(_))
}

fn summary_text(row: Option<Entry>) -> Option<String> {
    row.map(|entry| match entry {
        Entry::Notice(text) => text,
        other => panic!("summary row must be a notice: {other:?}"),
    })
}

// Covers: one summary per hidden run, on the run's first entry, counting only
// tools; visible entries split runs and tool-free runs paint nothing.
// Owner: zen tool-run grouping.
#[test]
fn summarizes_each_hidden_run_at_its_start() {
    let entries = vec![
        Entry::Assistant("before".into()),
        Entry::Reasoning(ReasoningEntry::new("plan")),
        tool(),
        Entry::Reasoning(ReasoningEntry::new("more")),
        tool(),
        tool(),
        Entry::Assistant("between".into()),
        tool(),
        Entry::Reasoning(ReasoningEntry::new("only reasoning after")),
        Entry::Assistant("after".into()),
        Entry::Reasoning(ReasoningEntry::new("no tools in this run")),
    ];

    let rows = (0..entries.len())
        .map(|index| summary_text(summary_row_at(&entries, index, hides_work)))
        .collect::<Vec<_>>();

    let mut expected = vec![None; entries.len()];
    expected[1] = summary_text(Some(summary_entry(3)));
    expected[7] = summary_text(Some(summary_entry(1)));
    assert_eq!(rows, expected);
}
