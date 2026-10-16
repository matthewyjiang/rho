//! Saving ACP sessions so `session/load` and the TUI's `/resume` can replay them.
//!
//! A new session writes no file until it first saves. Hosts open sessions
//! they never prompt (T3 Code's provider refresh runs `session/new` every few
//! minutes), and each would otherwise leave an empty row in `/resume`. The
//! TUI defers its file the same way.
//!
//! Hosts restart agents by killing them: T3 Code sends SIGTERM, then SIGKILL
//! one second later. `session/load` must still replay the whole conversation,
//! so a turn saves at every SDK step boundary (before each provider request
//! and after each model reply), not only when it ends. The turn writes the
//! user's message before it starts, so replay shows what was asked even when
//! the turn never finished, and a prompted session is never empty on disk.
//! Each save adds the turn's new history rows (tool
//! calls, results, interim replies) to the display transcript, as the TUI
//! does, so a resumed session shows the work and not only the final answer.
//! Background results the run applied are saved as the rows the host was
//! shown for them, in place of the model's copy.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use rho_sdk::{
    model::{Message, SemanticMessage},
    SessionId, SessionSnapshot, SessionStore, SessionStoreFuture, UserInput,
};

use super::SessionHost;
use crate::{
    added_dirs::AddedDirs, app::active_prompt::ActivePrompt, session::Session as StoredSession,
    tools::todo::TodoState,
};

/// Creates a new session's file on its first save.
pub(super) type CreateStoredSession = Box<dyn Fn() -> anyhow::Result<StoredSession> + Send + Sync>;

/// A host's workspace and session file. The file is absent until the first
/// save of a new session.
pub(super) struct HostStorage {
    cwd: PathBuf,
    file: SessionFile,
}

enum SessionFile {
    Unsaved(CreateStoredSession),
    Saved(StoredSession),
}

impl HostStorage {
    /// Storage for a new session `id` in `cwd`, created on first save.
    pub(super) fn deferred(
        cwd: PathBuf,
        id: String,
        agent_id: String,
        fingerprint: String,
    ) -> Self {
        let file_cwd = cwd.clone();
        Self::unsaved(
            cwd,
            Box::new(move || {
                StoredSession::create_with_id(&file_cwd, &id, &agent_id, &fingerprint)
            }),
        )
    }

    /// Storage in `cwd` whose file `create` makes on first save.
    pub(super) fn unsaved(cwd: PathBuf, create: CreateStoredSession) -> Self {
        Self {
            cwd,
            file: SessionFile::Unsaved(create),
        }
    }

    /// Storage for a session already on disk.
    pub(super) fn saved(stored: StoredSession) -> Self {
        Self {
            cwd: stored.cwd().to_path_buf(),
            file: SessionFile::Saved(stored),
        }
    }

    /// The session's workspace directory.
    pub(super) fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The session file, if it exists yet.
    pub(super) fn file(&self) -> Option<&StoredSession> {
        match &self.file {
            SessionFile::Saved(stored) => Some(stored),
            SessionFile::Unsaved(_) => None,
        }
    }
}

impl SessionHost {
    /// The session file, created now if this is the session's first save.
    pub(super) fn stored(&mut self) -> anyhow::Result<&StoredSession> {
        if let SessionFile::Unsaved(create) = &self.storage.file {
            let stored = create()?;
            self.built.tools.bind_recall(Some(&stored));
            self.storage.file = SessionFile::Saved(stored);
        }
        match &self.storage.file {
            SessionFile::Saved(stored) => Ok(stored),
            SessionFile::Unsaved(_) => unreachable!("the file was created above"),
        }
    }

    /// Writes the user's message, then saves the coming turn at every step
    /// boundary. Call while idle, with the message as shown (`display`) and
    /// as the run sends it (`model`); the returned checkpoint settles the
    /// turn in [`Self::persist_turn`].
    pub(super) fn install_turn_checkpoint(
        &mut self,
        display: &UserInput,
        model: &UserInput,
    ) -> anyhow::Result<TurnCheckpoint> {
        let snapshot = self.durable_snapshot();
        let stored = self.stored()?.clone();
        if !self.is_replaced() {
            stored.save_snapshot(&snapshot, &[Message::User(display.blocks().to_vec())])?;
        }
        let checkpoint = TurnCheckpoint(Arc::new(CheckpointStore {
            stored,
            todo: self.built.tools.todo_state(),
            prompt: self.built.prompt.clone(),
            added_dirs: self.added_dirs.clone(),
            replaced: Arc::clone(&self.replaced),
            display: Mutex::new(TurnDisplay::new(
                Message::User(model.blocks().to_vec()),
                snapshot.history().len(),
            )),
        }));
        self.built
            .session
            .set_checkpoint_store(Some(checkpoint.0.clone()))?;
        Ok(checkpoint)
    }

    /// Saves the settled turn with the display rows its step checkpoints
    /// have not written yet. `assistant_text` is the reply to show when the
    /// turn's history rows could not be followed.
    pub(super) fn persist_turn(
        &mut self,
        checkpoint: &TurnCheckpoint,
        assistant_text: Option<&str>,
    ) -> anyhow::Result<()> {
        if self.is_replaced() {
            return Ok(());
        }
        let snapshot = self.durable_snapshot();
        let mut display = checkpoint.0.lock_display();
        let mut pending = display.pending_rows(snapshot.history(), SaveScope::Settle);
        if !display.follows(snapshot.history()) {
            pending.rows.extend(
                assistant_text
                    .filter(|text| !text.is_empty())
                    .map(Message::assistant_text),
            );
        }
        self.stored()?.save_snapshot(&snapshot, &pending.rows)?;
        display.mark_written(pending);
        Ok(())
    }

    /// Saves what a failed turn already showed, such as background results
    /// delivered before the failure. The turn's own error is the one to
    /// report, so a failed save is only logged.
    pub(super) fn persist_failed_turn(&mut self, checkpoint: &TurnCheckpoint) {
        if let Err(error) = self.persist_turn(checkpoint, /*assistant_text*/ None) {
            tracing::warn!(%error, "could not save failed ACP turn");
        }
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

impl TurnCheckpoint {
    /// Records the row shown for a background result the run applied as
    /// `input`, so saves write it in place of the model's copy.
    pub(super) fn record_display(&self, input: &UserInput, shown: Message) {
        self.0
            .lock_display()
            .applied
            .push_back((Message::User(input.blocks().to_vec()), shown));
    }
}

struct CheckpointStore {
    stored: StoredSession,
    todo: TodoState,
    prompt: ActivePrompt,
    added_dirs: AddedDirs,
    replaced: Arc<AtomicBool>,
    display: Mutex<TurnDisplay>,
}

impl CheckpointStore {
    fn lock_display(&self) -> std::sync::MutexGuard<'_, TurnDisplay> {
        self.display
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save_now(&self, snapshot: SessionSnapshot) -> anyhow::Result<()> {
        if self.replaced.load(Ordering::Acquire) {
            return Ok(());
        }
        let snapshot = decorate(&self.prompt, &self.todo, &self.added_dirs, snapshot);
        let mut display = self.lock_display();
        let pending = display.pending_rows(snapshot.history(), SaveScope::Step);
        self.stored.save_snapshot(&snapshot, &pending.rows)?;
        display.mark_written(pending);
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

/// The display rows one turn has and has not written.
///
/// History from `history_start` on is this turn: the model's copy of the
/// user's message, then everything the run appends. The run pushes that copy
/// before its first save, and the user's message as shown is written when
/// the turn starts, so saves add only the appended rows, as they are,
/// except that background results show the rows recorded for them.
/// A compaction can rewrite history before or during the turn, so once the
/// model's copy is no longer at `history_start`, appended rows are lost and
/// the turn falls back to its recorded results and final reply.
#[derive(Debug)]
struct TurnDisplay {
    /// The model's copy of the user's message.
    input: Message,
    history_start: usize,
    followed: Followed,
    /// Background results applied and not yet written: the model's copy
    /// and the row shown for it, in the order the run applied them.
    applied: VecDeque<(Message, Message)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Followed {
    /// History extends this turn; this many rows after the input are written.
    Tracking { written: usize },
    /// History stopped extending this turn.
    Lost,
}

/// Which save is writing the turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveScope {
    /// A step checkpoint mid-run. A background result the pump has not
    /// recorded yet may still get its row, so the save stops before it.
    Step,
    /// The turn is over: write everything, including inputs that never got
    /// a recorded row.
    Settle,
}

/// What one save writes, and the display state once it lands.
struct PendingRows {
    rows: Vec<Message>,
    applied_written: usize,
    followed: Followed,
}

impl TurnDisplay {
    fn new(input: Message, history_start: usize) -> Self {
        Self {
            input,
            history_start,
            followed: Followed::Tracking { written: 0 },
            applied: VecDeque::new(),
        }
    }

    /// Rows a save of `history` must add to the display transcript.
    fn pending_rows(&self, history: &[Message], scope: SaveScope) -> PendingRows {
        let mut rows = Vec::new();
        let mut applied = self.applied.iter().peekable();
        let Some((appended, written)) = self.appended(history) else {
            let applied_written = match scope {
                SaveScope::Step => 0,
                SaveScope::Settle => {
                    rows.extend(applied.map(|(_, shown)| shown.clone()));
                    self.applied.len()
                }
            };
            return PendingRows {
                rows,
                applied_written,
                followed: Followed::Lost,
            };
        };
        let mut taken = 0;
        for row in appended {
            if let Some((_, shown)) = applied.next_if(|(model, _)| model == row) {
                rows.push(shown.clone());
            } else if scope == SaveScope::Step && matches!(row.semantic(), SemanticMessage::User(_))
            {
                break;
            } else {
                rows.push(row.clone());
            }
            taken += 1;
        }
        PendingRows {
            rows,
            applied_written: self.applied.len() - applied.len(),
            followed: Followed::Tracking {
                written: written + taken,
            },
        }
    }

    /// Records that a save wrote `pending`'s rows.
    fn mark_written(&mut self, pending: PendingRows) {
        self.applied.drain(..pending.applied_written);
        self.followed = pending.followed;
    }

    /// Whether `history` still extends this turn. When it does not, the
    /// turn's reply must be shown from the run outcome instead.
    fn follows(&self, history: &[Message]) -> bool {
        self.appended(history).is_some()
    }

    /// The rows after this turn's input that no save has written, and how
    /// many before them were. `None` once history no longer extends this turn.
    fn appended<'h>(&self, history: &'h [Message]) -> Option<(&'h [Message], usize)> {
        let Followed::Tracking { written } = self.followed else {
            return None;
        };
        if history.get(self.history_start) != Some(&self.input) {
            return None;
        }
        let rows = history.get(self.history_start + 1 + written..)?;
        Some((rows, written))
    }
}

#[cfg(test)]
#[path = "session_host_persist_tests.rs"]
mod tests;
