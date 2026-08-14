use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFamily, ToolHeader, ToolStatus};

use super::{latest_toggle_target, tool_target_at_line, HistoryItem, ToggleTarget};
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

fn tool_entry(card: ToolCard) -> ToolEntry {
    ToolEntry {
        card,
        expanded: false,
        image: None,
        started_at: None,
    }
}

fn paint_height(item: &HistoryItem<'_>, width: usize, max_tool_output_lines: usize) -> usize {
    item.paint_lines(width, max_tool_output_lines).len()
}

// Covers: click maps onto the card that owns the line, including spacer
// Owner: attach tool hit-test
#[test]
fn tool_target_at_line_maps_header_body_spacer_and_neighbors() {
    let long = Entry::Tool(tool_entry(long_card()));
    let notice = Entry::Notice("after".into());
    let pending = tool_entry(short_card());
    let items = [
        HistoryItem::Transcript {
            index: 0,
            entry: &long,
        },
        HistoryItem::Transcript {
            index: 1,
            entry: &notice,
        },
        HistoryItem::Pending {
            key: "live",
            tool: &pending,
        },
    ];
    let long_height = paint_height(&items[0], 80, 10);
    let notice_height = paint_height(&items[1], 80, 10);

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
                [
                    HistoryItem::Transcript {
                        index: 0,
                        entry: &long,
                    },
                    HistoryItem::Transcript {
                        index: 1,
                        entry: &notice,
                    },
                    HistoryItem::Pending {
                        key: "live",
                        tool: &pending,
                    },
                ],
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
    let finished = Entry::Tool(tool_entry(long_card()));
    let pending = tool_entry(long_card());
    assert_eq!(
        latest_toggle_target(
            [
                HistoryItem::Transcript {
                    index: 0,
                    entry: &finished,
                },
                HistoryItem::Pending {
                    key: "live",
                    tool: &pending,
                },
            ],
            80,
            10,
        ),
        Some(ToggleTarget::Pending("live".into()))
    );
}

// Covers: ctrl+o must not skip a non-expandable latest pending card
// Owner: attach tool hit-test
#[test]
fn latest_toggle_target_does_not_skip_non_expandable_pending() {
    let finished = Entry::Tool(tool_entry(long_card()));
    let pending = tool_entry(short_card());
    assert_eq!(
        latest_toggle_target(
            [
                HistoryItem::Transcript {
                    index: 0,
                    entry: &finished,
                },
                HistoryItem::Pending {
                    key: "live",
                    tool: &pending,
                },
            ],
            80,
            10,
        ),
        None
    );
}
