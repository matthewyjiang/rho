//! Shared checklist facts for native and external runtime cards.

use rho_tools::tool_card::{ToolCard, ToolFact};

#[derive(Clone, Copy, Debug)]
pub(crate) enum ChecklistStatus {
    Pending,
    InProgress,
    Completed,
}

/// Keep every item available for expansion; the generic card renderer owns
/// the collapsed terminal-row budget.
pub(crate) fn push_checklist_facts<'a>(
    card: &mut ToolCard,
    items: impl Iterator<Item = (&'a str, ChecklistStatus)>,
) {
    for (text, status) in items {
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
}
