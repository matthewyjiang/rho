//! Exact checklist authority shared by direct and codemode tool execution.
//!
//! Snapshot metadata owns durability and branching. The model sees the list in
//! its own `todo` calls; once compaction rewrites those away, the compactor
//! restates the list once at the end of the replacement history. Nothing is
//! projected per request: a suffix that moves every request defeats provider
//! prompt caching that marks only the request tail.

use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, Mutex},
};

use rho_sdk::{
    model::{context::estimate_context_tokens, ContentBlock, Message, ToolSpec},
    CompactionOutput, Error, SessionSnapshot,
};

use super::TodoList;

const METADATA_KEY: &str = "rho.todo.v1";
const RESTATEMENT_HEADER: &str = "[task checklist restated after compaction] Your latest successful todo replacement, which a later todo call replaces. This is context, not a new request.";

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
    /// state with its replacement, even if a later tool update races with it.
    pub(crate) fn checkpoint(&self) -> TodoCheckpoint {
        TodoCheckpoint {
            list: self.list(),
            restate: true,
        }
    }
}

/// Checklist state captured for one compaction.
pub(crate) struct TodoCheckpoint {
    list: Option<TodoList>,
    restate: bool,
}

impl TodoCheckpoint {
    /// The restatement [`Self::retain`] may add, for sizing the replacement.
    pub(crate) fn restatement(&self) -> Option<Message> {
        self.list.as_ref().and_then(restatement)
    }

    /// Commit metadata only, for a replacement with no room for the list.
    pub(crate) fn without_restatement(self) -> Self {
        Self {
            restate: false,
            ..self
        }
    }

    /// Size the exact history `retain` would commit, including its unchanged
    /// output fast path. Old restatements and a surviving native call must not
    /// be counted alongside a restatement that will never be appended.
    pub(crate) fn estimate_retained_tokens(
        &self,
        compacted: &[Message],
        replacement: &[Message],
        tools: &[ToolSpec],
    ) -> u64 {
        estimate_context_tokens(&self.retained_messages(compacted, replacement), tools)
    }

    fn retained_messages<'a>(
        &self,
        compacted: &[Message],
        replacement: &'a [Message],
    ) -> Cow<'a, [Message]> {
        if replacement == compacted {
            return Cow::Borrowed(replacement);
        }
        let mut messages: Vec<_> = replacement
            .iter()
            .filter(|message| !is_restatement(message))
            .cloned()
            .collect();
        if let Some(list) = self.list.as_ref().filter(|_| self.restate) {
            if successful_native_list(&messages).as_ref() != Some(list) {
                messages.extend(restatement(list));
            }
        }
        Cow::Owned(messages)
    }

    /// Commits the captured list as snapshot metadata alongside `output`.
    ///
    /// Earlier restatements are dropped. When the replacement no longer ends
    /// its `todo` calls with this exact list (a summary removed them, or the
    /// latest update ran inside codemode), one restatement is appended so the
    /// newest state comes last. Output identical to the compacted history is
    /// returned as is: rewriting it would only invalidate the prompt cache.
    pub(crate) fn retain(
        self,
        compacted: &[Message],
        output: CompactionOutput,
    ) -> Result<CompactionOutput, Error> {
        let metadata = serde_json::to_string(&self.list).expect("todo lists serialize");
        match self.retained_messages(compacted, output.messages()) {
            Cow::Borrowed(_) => Ok(output.with_metadata(METADATA_KEY, metadata)),
            Cow::Owned(messages) => CompactionOutput::with_usage(messages, output.usage().clone())
                .map(|output| output.with_metadata(METADATA_KEY, metadata)),
        }
    }
}

/// Whether `message` is a compaction restatement of the checklist. This is
/// attribution only: a forged copy is dropped at the next compaction, nothing more.
pub(crate) fn is_restatement(message: &Message) -> bool {
    matches!(
        message,
        Message::User(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(header), ContentBlock::Text(_)] if header == RESTATEMENT_HEADER)
    )
}

/// History message restating `list`; `None` for a cleared list.
fn restatement(list: &TodoList) -> Option<Message> {
    (!list.todos.is_empty()).then(|| {
        Message::User(vec![
            ContentBlock::Text(RESTATEMENT_HEADER.into()),
            ContentBlock::Text(serde_json::to_string(list).expect("todo lists serialize")),
        ])
    })
}

fn decorate(snapshot: SessionSnapshot, list: Option<TodoList>) -> SessionSnapshot {
    snapshot.with_metadata(
        METADATA_KEY,
        serde_json::to_string(&list).expect("todo lists serialize"),
    )
}

/// The latest list a native `todo` call in `history` replaced successfully.
/// Also the compatibility restore for sessions written before snapshot
/// metadata existed. A proposal alone or a failed/interrupted result does not count.
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
