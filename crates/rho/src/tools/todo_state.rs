//! Exact checklist authority shared by direct and codemode tool execution.
//!
//! Snapshot metadata owns durability and branching. Request-only context is a
//! projection; neither user input nor model-written summaries own this state.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    model::{ContentBlock, Message},
    CompactionOutput, SessionSnapshot,
};

use super::TodoList;

const METADATA_KEY: &str = "rho.todo.v1";

#[derive(Clone, Debug, Default)]
pub(crate) struct TodoState(Arc<Mutex<Option<TodoList>>>);

impl TodoState {
    pub(crate) fn list(&self) -> Option<TodoList> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn replace(&self, list: Option<TodoList>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = list;
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
        self.replace(list);
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
        let list = self.list();
        TodoCheckpoint {
            messages: context_messages(list.as_ref()),
            metadata: serde_json::to_string(&list).expect("todo lists serialize"),
        }
    }
}

impl rho_sdk::RequestContext for TodoState {
    fn messages(&self, _session_id: &rho_sdk::SessionId) -> Vec<Message> {
        context_messages(self.list().as_ref())
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
