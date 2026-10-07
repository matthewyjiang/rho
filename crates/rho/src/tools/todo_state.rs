//! Exact checklist authority shared by direct and codemode tool execution.
//!
//! Snapshot metadata owns durability and branching. Request-only context is a
//! projection; neither user input nor model-written summaries own this state.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    model::{
        context::{estimate_context_tokens, estimate_messages_tokens},
        ContentBlock, Message, ToolSpec,
    },
    tool::{ToolError, ToolErrorKind},
    CompactionOutput, SessionSnapshot,
};

use super::TodoList;

const METADATA_KEY: &str = "rho.todo.v1";

#[derive(Clone, Debug, Default)]
pub(crate) struct TodoState(Arc<Mutex<State>>);

#[derive(Debug, Default)]
struct State {
    list: Option<TodoList>,
    budget: Option<ContextBudget>,
}

#[derive(Clone, Copy, Debug)]
struct ContextBudget {
    limit: u64,
    request_tokens: u64,
    history_tokens: u64,
}

impl ContextBudget {
    fn asked(self, messages: &[Message]) -> u64 {
        self.request_tokens
            .saturating_add(self.history_tokens)
            .saturating_add(estimate_messages_tokens(messages))
    }
}

impl TodoState {
    pub(crate) fn list(&self) -> Option<TodoList> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .list
            .clone()
    }

    pub(crate) fn replace(&self, list: Option<TodoList>) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .list = list;
    }

    /// Reserve the actual history and advertised schemas, not a guessed tail
    /// allowance. Compaction refreshes this against its concrete replacement;
    /// turn/model refreshes must also account for history so suspension sticks.
    pub(crate) fn set_context_budget(
        &self,
        window: Option<u64>,
        history: &[Message],
        tools: &[ToolSpec],
    ) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .budget = window.map(|limit| ContextBudget {
            limit,
            request_tokens: estimate_context_tokens(&[], tools),
            history_tokens: estimate_messages_tokens(history),
        });
    }

    /// Validate and commit under the same lock so a nested update cannot race
    /// another replacement or a host budget refresh.
    pub(crate) fn try_replace(&self, list: TodoList) -> Result<(), ToolError> {
        let messages = context_messages(Some(&list));
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(budget) = state.budget {
            let asked = budget.asked(&messages);
            if asked > budget.limit {
                return Err(ToolError::new(ToolErrorKind::InvalidArguments, format!(
                    "todo mandatory context budget exceeded: limit {} estimated tokens, asked {asked}; shorten or clear the checklist",
                    budget.limit,
                )));
            }
        }
        state.list = Some(list);
        Ok(())
    }

    pub(crate) fn restore(&self, snapshot: &SessionSnapshot) {
        let list = if let Some(value) = snapshot.metadata().get(METADATA_KEY) {
            // An explicit null prevents fallback from resurrecting an old list.
            match serde_json::from_str(value) {
                Ok(list) => list,
                Err(error) => {
                    tracing::warn!(%error, "could not restore todo snapshot metadata");
                    None
                }
            }
        } else {
            successful_native_list(snapshot.history())
        };
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(budget) = state.budget.as_mut() {
            budget.history_tokens = estimate_messages_tokens(snapshot.history());
        }
        state.list = list;
    }

    pub(crate) fn decorate(&self, snapshot: SessionSnapshot) -> SessionSnapshot {
        decorate(snapshot, self.list())
    }

    /// Headless SDK checkpointing also crosses this host-owned state seam.
    pub(crate) fn checkpoint_store(
        &self,
        inner: Arc<dyn rho_sdk::SessionStore>,
    ) -> Arc<dyn rho_sdk::SessionStore> {
        Arc::new(TodoCheckpointStore {
            inner,
            todo: self.clone(),
        })
    }

    /// Capture once before compaction awaits a provider. The SDK commits this
    /// metadata with its replacement, even if a later tool update races with it.
    pub(crate) fn checkpoint(&self) -> TodoCheckpoint {
        let state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        TodoCheckpoint {
            messages: projected_messages(&state),
            metadata: serde_json::to_string(&state.list).expect("todo lists serialize"),
        }
    }
}

impl rho_sdk::RequestContext for TodoState {
    fn messages(&self, _session_id: &rho_sdk::SessionId) -> Vec<Message> {
        projected_messages(
            &self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

pub(crate) struct TodoCheckpoint {
    messages: Vec<Message>,
    metadata: String,
}

impl TodoCheckpoint {
    pub(crate) fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub(crate) fn retain(self, output: CompactionOutput) -> CompactionOutput {
        output.with_metadata(METADATA_KEY, self.metadata)
    }
}

/// Older snapshots, growing history, or a smaller model can exceed the budget. Keep
/// the exact list durable and readable in /todo, but suspend its full projection
/// so a provider turn can reach the todo tool to shorten or clear it.
fn projected_messages(state: &State) -> Vec<Message> {
    let Some(list) = state.list.as_ref() else {
        return Vec::new();
    };
    let messages = context_messages(Some(list));
    if let Some(budget) = state.budget {
        let asked = budget.asked(&messages);
        if asked > budget.limit {
            let recovery = vec![Message::model_context(format!(
                "The saved task checklist is too large to project: mandatory context budget limit {} estimated tokens, asked {asked}. The exact list is preserved in /todo and session storage. Use todo to replace it with a short checklist or clear it, or select a larger-context model.",
                budget.limit,
            ))];
            tracing::warn!(
                limit = budget.limit,
                asked,
                "suspending oversized todo request context"
            );
            // Even the recovery notice must not exceed the mandatory budget.
            return if budget.asked(&recovery) <= budget.limit {
                recovery
            } else {
                Vec::new()
            };
        }
    }
    messages
}

fn context_messages(list: Option<&TodoList>) -> Vec<Message> {
    list.map_or_else(Vec::new, |list| {
        let json = serde_json::to_string(list).expect("todo lists serialize");
        vec![Message::model_context(format!(
            "Current task checklist (exact latest successful todo replacement):\n{json}"
        ))]
    })
}

fn decorate(snapshot: SessionSnapshot, list: Option<TodoList>) -> SessionSnapshot {
    snapshot.with_metadata(
        METADATA_KEY,
        serde_json::to_string(&list).expect("todo lists serialize"),
    )
}

/// Compatibility for sessions written before exact snapshot metadata existed.
/// A proposal alone or a failed/interrupted result must not replace the list.
fn successful_native_list(history: &[Message]) -> Option<TodoList> {
    let mut calls = HashMap::new();
    let mut latest = None;
    for message in history {
        if let Some(blocks) = message.completed_assistant_content() {
            for block in blocks {
                match block {
                    ContentBlock::ToolCall(call) if call.name == "todo" => {
                        calls.insert(call.id.clone(), call.arguments.clone());
                    }
                    _ => {}
                }
            }
        }
        if let Message::ToolResult(result) = message {
            let list = calls
                .remove(&result.id)
                .filter(|_| result.ok)
                .and_then(|arguments| TodoList::parse(arguments).ok());
            if let Some(list) = list {
                latest = Some(list);
            }
        }
    }
    latest
}

struct TodoCheckpointStore {
    inner: Arc<dyn rho_sdk::SessionStore>,
    todo: TodoState,
}

impl rho_sdk::SessionStore for TodoCheckpointStore {
    fn load<'a>(
        &'a self,
        id: &'a rho_sdk::SessionId,
    ) -> rho_sdk::SessionStoreFuture<'a, Option<SessionSnapshot>> {
        self.inner.load(id)
    }

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> rho_sdk::SessionStoreFuture<'a, ()> {
        self.inner.save(self.todo.decorate(snapshot))
    }
}

#[cfg(test)]
#[path = "todo_state_tests.rs"]
mod tests;
