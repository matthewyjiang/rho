//! Stateless full-list task checklists; session call arguments own persistence.

use std::sync::Arc;

use rho_sdk::tool::{
    OperationKind, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation,
    ToolMetadata, ToolOutput, ToolSecurity,
};
use serde::Deserialize;

// Product tripwire: five times the default ten-row collapsed card budget, not a
// storage allocation. Larger projects should keep this list at milestone level.
const MAX_TODOS: usize = 50;

pub(super) fn sdk_bundle() -> super::sdk_registry::StaticToolBundle {
    super::sdk_registry::StaticToolBundle::new(vec![Arc::new(TodoTool)])
}

pub(super) struct TodoTool;

impl Tool for TodoTool {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: "todo".into(),
            description: "Track a short task checklist shown to the user. Replace the whole list on every call; include all items you want to keep. Use for multi-step work with three or more steps, and skip trivial single-step tasks. Keep exactly one item in_progress while working, and update the list as soon as a step finishes. Mark finished items completed.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "Complete replacement checklist, at most 50 items; use an empty list to clear it",
                        "maxItems": MAX_TODOS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string", "minLength": 1},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                            },
                            "required": ["content", "status"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["todos"],
                "additionalProperties": false
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let list = TodoList::parse(invocation.into_arguments())?;
            Ok(ToolOutput::text(list.summary())
                .metadata(ToolMetadata::new().operation(OperationKind::Other("todo".into()))))
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TodoList {
    pub(crate) todos: Vec<TodoItem>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TodoItem {
    pub(crate) content: String,
    pub(crate) status: TodoStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoList {
    pub(crate) fn parse(arguments: serde_json::Value) -> Result<Self, ToolError> {
        let invalid = |message| ToolError::new(ToolErrorKind::InvalidArguments, message);
        let list: Self = serde_json::from_value(arguments)
            .map_err(|error| invalid(format!("invalid todo arguments: {error}")))?;
        if list.todos.len() > MAX_TODOS {
            return Err(invalid(format!(
                "todo list has {} items, limit {MAX_TODOS}",
                list.todos.len()
            )));
        }
        for (index, todo) in list.todos.iter().enumerate() {
            if todo.content.trim().is_empty() {
                return Err(invalid(format!(
                    "todo item {} content must not be empty",
                    index + 1
                )));
            }
        }
        let in_progress = list
            .todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::InProgress)
            .count();
        if in_progress > 1 {
            return Err(invalid(format!(
                "todo list has {in_progress} in_progress items, limit 1"
            )));
        }
        Ok(list)
    }

    pub(crate) fn summary(&self) -> String {
        let mut completed = 0;
        let mut in_progress = 0;
        let mut pending = 0;
        for todo in &self.todos {
            match todo.status {
                TodoStatus::Completed => completed += 1,
                TodoStatus::InProgress => in_progress += 1,
                TodoStatus::Pending => pending += 1,
            }
        }
        format!(
            "{} todos: {completed} completed, {in_progress} in progress, {pending} pending",
            self.todos.len()
        )
    }
}

#[cfg(test)]
#[path = "todo_tests.rs"]
mod tests;
