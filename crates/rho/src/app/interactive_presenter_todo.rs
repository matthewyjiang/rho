//! Native checklist cards are rebuilt from call arguments, including on resume.

use rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus};

use crate::{
    presentation::checklist::{push_checklist_facts, ChecklistStatus},
    tools::todo::TodoStatus,
};

pub(super) fn card(arguments: &serde_json::Value, status: ToolStatus) -> ToolCard {
    let mut card = ToolCard::new(status, ToolFamily::Default, ToolHeader::call("todo", None));
    if let Some(todos) = arguments.get("todos").and_then(serde_json::Value::as_array) {
        // Repaired streaming JSON can contain a trailing item without content or
        // a complete status. Project usable items independently so that fragment
        // never hides preceding items. Execution validation stays in TodoList.
        push_checklist_facts(
            &mut card,
            todos.iter().filter_map(|todo| {
                let content = todo.get("content")?.as_str()?;
                if content.trim().is_empty() {
                    return None;
                }
                let status = serde_json::from_value(todo.get("status")?.clone()).ok()?;
                let status = match status {
                    TodoStatus::Pending => ChecklistStatus::Pending,
                    TodoStatus::InProgress => ChecklistStatus::InProgress,
                    TodoStatus::Completed => ChecklistStatus::Completed,
                };
                Some((content, status))
            }),
        );
    }
    card
}

pub(super) fn finished_card(arguments: &serde_json::Value, content: &str, ok: bool) -> ToolCard {
    if ok {
        return card(arguments, ToolStatus::Ok);
    }
    // A rejected proposal is not the checklist, and its error must remain
    // visible even when the generic card renderer collapses long output.
    let mut card = ToolCard::new(
        ToolStatus::Error,
        ToolFamily::Default,
        ToolHeader::call("todo", None),
    );
    super::format::push_error_output(&mut card, content);
    card
}
