//! Saving ACP sessions so `session/load` can replay them.
//!
//! Hosts restart agents by killing them: T3 Code sends SIGTERM, then SIGKILL
//! one second later. `session/load` must still replay the whole conversation,
//! so a turn saves at every SDK step boundary (before each provider request
//! and after each model reply), not only when it ends. The first save also
//! writes the user's message, so replay shows what was asked even when the
//! turn never finished.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use rho_sdk::{
    model::Message, SessionId, SessionSnapshot, SessionStore, SessionStoreFuture, UserInput,
};

use super::SessionHost;
use crate::{
    added_dirs::AddedDirs, app::active_prompt::ActivePrompt, session::Session as StoredSession,
    tools::todo::TodoState,
};

impl SessionHost {
    /// Saves the coming turn at every step boundary. Call while idle; the
    /// returned checkpoint settles the turn in [`Self::persist_turn`].
    pub(super) fn install_turn_checkpoint(
        &self,
        display: &UserInput,
    ) -> anyhow::Result<TurnCheckpoint> {
        let checkpoint = TurnCheckpoint(Arc::new(CheckpointStore {
            stored: self.stored.clone(),
            todo: self.built.tools.todo_state(),
            prompt: self.built.prompt.clone(),
            added_dirs: self.added_dirs.clone(),
            replaced: Arc::clone(&self.replaced),
            unsaved_display: Mutex::new(vec![Message::User(display.blocks().to_vec())]),
        }));
        self.built
            .session
            .set_checkpoint_store(Some(checkpoint.0.clone()))?;
        Ok(checkpoint)
    }

    /// Saves the settled turn with the display rows its step checkpoints
    /// have not written yet.
    pub(super) fn persist_turn(
        &self,
        checkpoint: &TurnCheckpoint,
        assistant_text: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.is_replaced() {
            return Ok(());
        }
        let mut display_tail = std::mem::take(&mut *checkpoint.0.lock_display());
        if let Some(text) = assistant_text.filter(|text| !text.is_empty()) {
            display_tail.push(Message::assistant_text(text));
        }
        self.stored
            .save_snapshot(&self.durable_snapshot(), &display_tail)
    }

    /// A replaced host must not write over its successor's session file.
    pub(super) fn is_replaced(&self) -> bool {
        self.replaced.load(Ordering::Acquire)
    }

    /// The live snapshot with the host state a later load restores.
    pub(super) fn durable_snapshot(&self) -> SessionSnapshot {
        decorate(
            &self.built.prompt,
            &self.built.tools.todo_state(),
            &self.added_dirs,
            self.built.session.snapshot(),
        )
    }
}

/// Adds the host state a load restores: model prompt, checklist, and added
/// directories.
fn decorate(
    prompt: &ActivePrompt,
    todo: &TodoState,
    added_dirs: &AddedDirs,
    snapshot: SessionSnapshot,
) -> SessionSnapshot {
    added_dirs.decorate(todo.decorate(prompt.decorate(snapshot)))
}

/// One turn's SDK checkpoint store, installed by
/// [`SessionHost::install_turn_checkpoint`].
pub(super) struct TurnCheckpoint(Arc<CheckpointStore>);

struct CheckpointStore {
    stored: StoredSession,
    todo: TodoState,
    prompt: ActivePrompt,
    added_dirs: AddedDirs,
    replaced: Arc<AtomicBool>,
    /// Display rows no save has written yet: the user's message until the
    /// first checkpoint lands.
    unsaved_display: Mutex<Vec<Message>>,
}

impl CheckpointStore {
    fn lock_display(&self) -> std::sync::MutexGuard<'_, Vec<Message>> {
        self.unsaved_display
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save_now(&self, snapshot: SessionSnapshot) -> anyhow::Result<()> {
        if self.replaced.load(Ordering::Acquire) {
            return Ok(());
        }
        let snapshot = decorate(&self.prompt, &self.todo, &self.added_dirs, snapshot);
        let mut display = self.lock_display();
        self.stored.save_snapshot(&snapshot, &display)?;
        display.clear();
        Ok(())
    }
}

impl SessionStore for CheckpointStore {
    fn load<'a>(&'a self, id: &'a SessionId) -> SessionStoreFuture<'a, Option<SessionSnapshot>> {
        SessionStore::load(&self.stored, id)
    }

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            self.save_now(snapshot)
                .map_err(|error| rho_sdk::Error::Persistence {
                    message: format!("{error:#}"),
                })
        })
    }
}

#[cfg(test)]
#[path = "session_host_persist_tests.rs"]
mod tests;
