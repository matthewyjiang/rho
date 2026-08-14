use std::borrow::Cow;

use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFamily, ToolHeader, ToolStatus};

use super::{entry_height, latest_toggle_target, tool_target_at_line, HistoryItem, ToggleTarget};
use crate::tui::{Entry, ToolEntry};

fn long_card() -> ToolCard {
    ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::shell("$", Some("seq 20".into())),
    )
    .with_body(ToolBody::Lines(
        (0..20).map(|i| format!("out-{i}")).collect(),
    ))
}

fn short_card() -> ToolCard {
    ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("echo", None),
    )
    .with_body(ToolBody::Lines(vec!["ok".into()]))
}

fn tool_entry(card: ToolCard) -> Entry {
    Entry::Tool(ToolEntry {
        card,
        expanded: false,
        image: None,
        started_at: None,
    })
}

fn items(entries: Vec<(Option<ToggleTarget>, Entry)>) -> Vec<HistoryItem<'static>> {
    entries
        .into_iter()
        .map(|(target, entry)| HistoryItem {
            target,
            entry: Cow::Owned(entry),
        })
        .collect()
}

// Covers: click maps onto the card that owns the line, including spacer
// Owner: attach tool hit-test
#[test]
fn tool_target_at_line_maps_header_body_spacer_and_neighbors() {
    let long = tool_entry(long_card());
    let notice = Entry::Notice("after".into());
    let short = tool_entry(short_card());
    let long_height = entry_height(&long, 80, 10);
    let notice_height = entry_height(&notice, 80, 10);
    let mapped = items(vec![
        (Some(ToggleTarget::Transcript(0)), long),
        (None, notice),
        (Some(ToggleTarget::Pending("live".into())), short),
    ]);

    let cases = [
        (0, Some(ToggleTarget::Transcript(0))),
        (1, Some(ToggleTarget::Transcript(0))),
        (long_height - 1, Some(ToggleTarget::Transcript(0))),
        (long_height, None),
        (long_height + notice_height, None),
        (usize::MAX, None),
    ];
    for (line, expected) in cases {
        assert_eq!(
            tool_target_at_line(
                mapped.iter().map(|item| HistoryItem {
                    target: item.target.clone(),
                    entry: Cow::Borrowed(item.entry.as_ref()),
                }),
                line,
                80,
                10
            ),
            expected,
            "line {line}"
        );
    }
}

// Covers: ctrl+o prefers the last toggleable pending card
// Owner: attach tool hit-test
#[test]
fn latest_toggle_target_prefers_pending() {
    let mapped = items(vec![
        (Some(ToggleTarget::Transcript(0)), tool_entry(long_card())),
        (
            Some(ToggleTarget::Pending("live".into())),
            tool_entry(long_card()),
        ),
    ]);
    assert_eq!(
        latest_toggle_target(
            mapped.iter().map(|item| HistoryItem {
                target: item.target.clone(),
                entry: Cow::Borrowed(item.entry.as_ref()),
            }),
            80,
            10,
        ),
        Some(ToggleTarget::Pending("live".into()))
    );
}
