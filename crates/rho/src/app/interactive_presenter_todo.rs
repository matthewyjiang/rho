//! Native checklist cards are rebuilt from call arguments, including on resume.

use rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus};

use crate::{
    presentation::checklist::{push_checklist_facts, ChecklistStatus},
    tools::todo::{TodoList, TodoStatus},
};

pub(super) fn card(arguments: &serde_json::Value, status: ToolStatus) -> ToolCard {
    let mut card = ToolCard::new(status, ToolFamily::Default, ToolHeader::call("todo", None));
    if let Ok(list) = TodoList::parse(arguments.clone()) {
        push_checklist_facts(
            &mut card,
            list.todos.iter().map(|todo| {
                let status = match todo.status {
                    TodoStatus::Pending => ChecklistStatus::Pending,
                    TodoStatus::InProgress => ChecklistStatus::InProgress,
                    TodoStatus::Completed => ChecklistStatus::Completed,
                };
                (todo.content.as_str(), status)
            }),
        );
    }
    card
}

pub(super) fn finished_card(arguments: &serde_json::Value, content: &str, ok: bool) -> ToolCard {
    let mut card = card(arguments, ToolStatus::from_finished(ok));
    if !ok {
        super::format::push_error_output(&mut card, content);
    }
    card
}
