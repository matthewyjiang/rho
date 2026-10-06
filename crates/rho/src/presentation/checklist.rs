//! Shared checklist facts for native and external runtime cards.

use rho_tools::tool_card::{ToolCard, ToolFact};

#[derive(Clone, Copy, Debug)]
pub(crate) enum ChecklistStatus {
    Pending,
    InProgress,
    Completed,
}

/// Retain Claude TodoWrite's existing ten-row preview budget. Overflow remains
/// visible as a count; the original list is still kept in the call arguments.
pub(crate) fn push_checklist_facts<'a>(
    card: &mut ToolCard,
    items: impl ExactSizeIterator<Item = (&'a str, ChecklistStatus)>,
) {
    const MAX_CHECKLIST_FACTS: usize = 10;
    let count = items.len();
    for (text, status) in items.take(MAX_CHECKLIST_FACTS) {
        if text.is_empty() {
            continue;
        }
        let marker = match status {
            ChecklistStatus::Completed => "☑",
            ChecklistStatus::InProgress => "◐",
            ChecklistStatus::Pending => "☐",
        };
        card.push_fact(ToolFact::Text {
            text: format!("{marker} {text}"),
        });
    }
    if count > MAX_CHECKLIST_FACTS {
        card.push_fact(ToolFact::Meta {
            text: format!("{} more", count - MAX_CHECKLIST_FACTS),
        });
    }
}
