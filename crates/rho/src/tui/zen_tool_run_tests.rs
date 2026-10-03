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

// Covers: one anchor per run of hidden work, counting only tools, split by visible entries.
// Owner: zen tool-run grouping.
#[test]
fn anchors_one_summary_per_hidden_run() {
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

    let summaries = (0..entries.len())
        .map(|index| summary_at(&entries, index, hides_work))
        .collect::<Vec<_>>();

    assert_eq!(
        summaries,
        [
            None,
            None,
            Some(3),
            None,
            None,
            None,
            None,
            Some(1),
            None,
            None,
            None
        ]
    );
}

// Covers: rebuild start rewinds over the hidden run ending before an index.
// Owner: zen tool-run grouping.
#[test]
fn run_start_rewinds_to_hidden_run_before_index() {
    let entries = vec![
        Entry::Assistant("before".into()),
        Entry::Reasoning(ReasoningEntry::new("plan")),
        tool(),
        tool(),
        Entry::Assistant("after".into()),
    ];

    let starts = (0..=entries.len() + 1)
        .map(|index| run_start(&entries, index, hides_work))
        .collect::<Vec<_>>();

    // Indices past the end clamp to the transcript length.
    assert_eq!(starts, [0, 1, 1, 1, 1, 5, 5]);
}
