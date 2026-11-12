//! Saving sessions so a host's reload (ACP `session/load`) and the TUI's
//! `/resume` can replay them.
//!
//! A new session writes no file until it first saves. Hosts open sessions
//! they never prompt (T3 Code's provider refresh runs `session/new` every few
//! minutes), and each would otherwise leave an empty row in `/resume`. The
//! TUI defers its file the same way.
//!
//! Hosts restart agents by killing them: T3 Code sends SIGTERM, then SIGKILL
//! one second later. A reload must still replay the whole conversation,
//! so a turn saves at every SDK step boundary (before each provider request
//! and after each model reply), not only when it ends. The first save also
//! writes the user's message, so replay shows what was asked even when the
//! turn never finished. Each save adds the turn's new history rows (tool
//! calls, results, interim replies) to the display transcript, as the TUI
//! does, so a resumed session shows the work and not only the final answer.
//! Background results the run applied are saved as the rows the host was
//! shown for them, in place of the model's copy.
//!
//! A refusal (ACP's `refusal` stop reason) promises that the refused prompt
//! and everything after it leave the next prompt. A turn that only produced
//! model replies is rolled back after it is saved: the session moves back to
//! the node it started from, so the refused turn stays in the file as an
//! abandoned branch that neither the model nor a reload sees.
//!
//! An [`SessionPersistence::Ephemeral`] session (`rho run --no-save`) never
//! writes a file: its turns run the same way and save nothing, and a refusal
//! rolls back only its live history.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rho_sdk::{
    model::{ContentBlock, Message, SemanticMessage},
    RunOutcome, SessionId, SessionSnapshot, SessionStore, SessionStoreFuture, UserInput,
};

use super::{
    events::InsertedMessage, left_out::LeftOut, saved_settings::SettingsRecord, SessionRuntime,
};
use crate::{
    added_dirs::AddedDirs,
    app::active_prompt::ActivePrompt,
    session::{
        tree::{DisplayRowOrigin, NodeId},
        Session as StoredSession, SessionPersistence, SessionProvenance,
    },
    tools::todo::TodoState,
};

/// Creates a new session's file on its first save.
pub(crate) type CreateStoredSession = Box<dyn Fn() -> anyhow::Result<StoredSession> + Send + Sync>;

/// A session's workspace and file. The file is absent until the first save
/// of a new session, and always for an ephemeral one.
pub(crate) struct SessionStorage {
    cwd: PathBuf,
    file: SessionFile,
}

enum SessionFile {
    Unsaved(CreateStoredSession),
    Saved(StoredSession),
    /// Never written; see [`SessionPersistence::Ephemeral`].
    Ephemeral,
}

impl SessionStorage {
    /// Storage for a new session `id` in `cwd`, created on first save and
    /// recording the `provenance` that opened it.
    pub(crate) fn deferred(
        cwd: PathBuf,
        id: String,
        agent_id: String,
        fingerprint: String,
        provenance: SessionProvenance,
    ) -> Self {
        let file_cwd = cwd.clone();
        Self::unsaved(
            cwd,
            Box::new(move || {
                StoredSession::create_with_id(
                    &file_cwd,
                    &id,
                    &agent_id,
                    &fingerprint,
                    provenance.clone(),
                )
            }),
        )
    }

    /// Storage in `cwd` whose file `create` makes on first save.
    pub(crate) fn unsaved(cwd: PathBuf, create: CreateStoredSession) -> Self {
        Self {
            cwd,
            file: SessionFile::Unsaved(create),
        }
    }

    /// Storage for a session already on disk.
    pub(crate) fn saved(stored: StoredSession) -> Self {
        Self {
            cwd: stored.cwd().to_path_buf(),
            file: SessionFile::Saved(stored),
        }
    }

    /// Storage in `cwd` for a session that is never written.
    pub(crate) fn ephemeral(cwd: PathBuf) -> Self {
        Self {
            cwd,
            file: SessionFile::Ephemeral,
        }
    }

    /// The session's workspace directory.
    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The session file, if it exists yet.
    pub(crate) fn file(&self) -> Option<&StoredSession> {
        match &self.file {
            SessionFile::Saved(stored) => Some(stored),
            SessionFile::Unsaved(_) | SessionFile::Ephemeral => None,
        }
    }

    /// Whether the session is ever written, which the children it starts
    /// follow.
    pub(crate) fn persistence(&self) -> SessionPersistence {
        match &self.file {
            SessionFile::Unsaved(_) | SessionFile::Saved(_) => SessionPersistence::Saved,
            SessionFile::Ephemeral => SessionPersistence::Ephemeral,
        }
    }
}

impl SessionRuntime {
    /// The session file, created now if this is the session's first save,
    /// or `None` for an ephemeral session, which saves nothing.
    pub(super) fn stored(&mut self) -> anyhow::Result<Option<&StoredSession>> {
        if let SessionFile::Unsaved(create) = &self.storage.file {
            let stored = create()?;
            self.built.tools.bind_recall(Some(&stored));
            self.storage.file = SessionFile::Saved(stored);
        }
        Ok(self.storage.file())
    }

    /// Saves the coming turn at every step boundary. Call while idle, with
    /// the message as shown (`display`) and as the run sends it (`model`);
    /// the returned checkpoint settles the turn in [`Self::persist_turn`].
    pub(super) fn install_turn_checkpoint(
        &mut self,
        display: &UserInput,
        model: &UserInput,
    ) -> anyhow::Result<TurnCheckpoint> {
        let stored = self.stored()?.cloned();
        let rollback = match &stored {
            Some(stored) => Rollback::ToNode {
                base: turn_base(stored, || self.durable_snapshot())?,
                stored: stored.clone(),
            },
            None => Rollback::LiveHistory,
        };
        let display = TurnDisplay::new(
            Message::User(display.blocks().to_vec()),
            Message::User(model.blocks().to_vec()),
            self.built.session.history().len(),
        );
        self.install_checkpoint(stored, rollback, display)
    }

    /// Saves a turn that continues the restored history at every step
    /// boundary, as [`Self::install_turn_checkpoint`] does for a prompt.
    /// The turn adds no user message and never rolls back.
    pub(super) fn install_continuation_checkpoint(&mut self) -> anyhow::Result<TurnCheckpoint> {
        let stored = self.stored()?.cloned();
        let history = self.built.session.history();
        let Some(last) = history.last() else {
            anyhow::bail!("could not continue session: it has no history");
        };
        let display = TurnDisplay::continuing(last.clone(), history.len() - 1);
        self.install_checkpoint(stored, Rollback::Never, display)
    }

    fn install_checkpoint(
        &mut self,
        stored: Option<StoredSession>,
        rollback: Rollback,
        display: TurnDisplay,
    ) -> anyhow::Result<TurnCheckpoint> {
        let checkpoint = TurnCheckpoint(Arc::new(CheckpointStore {
            stored,
            rollback,
            state: self.durable_state(),
            display: Mutex::new(display),
        }));
        self.built
            .session
            .set_checkpoint_store(Some(checkpoint.0.clone()))?;
        Ok(checkpoint)
    }

    /// Saves the turn's user message before its run starts, and returns the
    /// message as inserted.
    pub(super) fn insert_user_message(
        &mut self,
        checkpoint: &TurnCheckpoint,
    ) -> anyhow::Result<InsertedMessage> {
        let mut display = checkpoint.0.lock_display();
        let Some(user) = display.unsaved_user.clone() else {
            return Ok(InsertedMessage::Unreplayed);
        };
        let (inserted, []) = self.save_prompt_rows(user, [])?;
        display.mark_user_written();
        Ok(inserted)
    }

    /// Saves the prompt's `user` message with the display rows `following`
    /// it as one node, and returns the rows a reload replays each message
    /// as, in order. An ephemeral runtime saves nothing, so its messages are
    /// unreplayed.
    pub(crate) fn save_prompt_rows<const N: usize>(
        &mut self,
        user: Message,
        following: [Message; N],
    ) -> anyhow::Result<(InsertedMessage, [InsertedMessage; N])> {
        let unreplayed = || {
            (
                InsertedMessage::Unreplayed,
                [(); N].map(|()| InsertedMessage::Unreplayed),
            )
        };
        let rows: Vec<Message> = std::iter::once(user).chain(following).collect();
        let snapshot = self.durable_snapshot();
        let Some(stored) = self.stored()? else {
            return Ok(unreplayed());
        };
        let node = stored.save_snapshot_node(&snapshot, &rows)?;
        let row = |row| {
            InsertedMessage::Saved(DisplayRowOrigin {
                node: node.clone(),
                row,
            })
        };
        Ok((row(0), std::array::from_fn(|index| row(index + 1))))
    }

    /// Saves the settled turn with the display rows its step checkpoints
    /// have not written yet. `assistant_text` is the reply to show when the
    /// turn's history rows could not be followed.
    pub(super) fn persist_turn(
        &mut self,
        checkpoint: &TurnCheckpoint,
        assistant_text: Option<&str>,
    ) -> anyhow::Result<()> {
        let snapshot = self.durable_snapshot();
        let Some(stored) = self.stored()?.cloned() else {
            return Ok(());
        };
        let mut display = checkpoint.0.lock_display();
        let mut pending = display.pending_rows(snapshot.history(), SaveScope::Settle);
        if !display.follows(snapshot.history()) {
            pending.rows.extend(
                assistant_text
                    .filter(|text| !text.is_empty())
                    .map(Message::assistant_text),
            );
        }
        stored.save_snapshot(&snapshot, &pending.rows)?;
        display.mark_written(pending);
        Ok(())
    }

    /// Saves a completed turn, then rolls back a refused prompt as a
    /// refusal requires. A refused turn that ran tools or took other input
    /// stays, since its effects are already real.
    pub(super) fn settle_turn(
        &mut self,
        checkpoint: &TurnCheckpoint,
        outcome: &RunOutcome,
    ) -> anyhow::Result<()> {
        self.persist_turn(checkpoint, Some(outcome.text()))?;
        if outcome.stop_reason() != rho_sdk::StopReason::Refusal {
            return Ok(());
        }
        let history = self.built.session.history();
        let Some(start) = checkpoint.0.lock_display().prompt_only_start(&history) else {
            return Ok(());
        };
        let (stored, base) = match &checkpoint.0.rollback {
            // A continuation added no prompt to roll back.
            Rollback::Never => return Ok(()),
            Rollback::LiveHistory => {
                // Nothing is saved, so only the live session forgets it.
                self.built
                    .session
                    .replace_history(history[..start].to_vec())?;
                return Ok(());
            }
            Rollback::ToNode { stored, base } => (stored, base),
        };
        // Durable first, as tree selection does: if the leaf commit fails, the
        // live session still matches the refusal branch the file selects.
        let refused_leaf = stored.with_session_tree(|tree| Ok(tree.active_leaf_id().cloned()))?;
        stored.set_leaf(base)?;
        if let Err(error) = self
            .built
            .session
            .replace_history(history[..start].to_vec())
        {
            if let Some(leaf) = refused_leaf {
                stored.set_leaf(&leaf)?;
            }
            return Err(error.into());
        }
        // A new node on the base keeps session state that changed since the base
        // was saved, such as the selected model, without the refused rows. If
        // it fails, the file still selects the base, which matches the live
        // history; only that session state waits for the next save.
        stored.save_snapshot(&self.durable_snapshot(), &[])
    }

    /// Saves what a failed turn already showed, such as background results
    /// delivered before the failure. The turn's own error is the one to
    /// report, so a failed save is only logged.
    pub(super) fn persist_failed_turn(&mut self, checkpoint: &TurnCheckpoint) {
        if let Err(error) = self.persist_turn(checkpoint, /*assistant_text*/ None) {
            tracing::warn!(%error, "could not save failed turn");
        }
    }

    /// The session state a later load restores, as it stands now.
    pub(super) fn durable_state(&self) -> DurableState {
        DurableState {
            prompt: self.built.prompt.clone(),
            todo: self.built.tools.todo_state(),
            code_mode: self.built.tools.code_mode().clone(),
            added_dirs: self.added_dirs.clone(),
            left_out: self.build.launch.left_out,
            auth: self.auth.clone(),
            settings: self.settings_record(),
        }
    }

    /// The live snapshot with the session state a later load restores.
    pub(super) fn durable_snapshot(&self) -> SessionSnapshot {
        self.durable_state().decorate(self.built.session.snapshot())
    }

    /// Saves the session's state now, if it has a file, as a revision that
    /// differs from its leaf only by state a load restores, such as its
    /// settings or what it leaves out. A new session's first save writes it.
    pub(crate) fn save_state(&mut self) -> anyhow::Result<()> {
        let Some(stored) = self.storage.file() else {
            return Ok(());
        };
        self.built.session.advance_revision()?;
        stored.save_snapshot(&self.durable_snapshot(), &[])
    }
}

/// The session state a load restores, which every durable save adds to the
/// SDK's snapshot. [`SessionRuntime::durable_state`] takes it from a
/// runtime.
pub(super) struct DurableState {
    pub(super) prompt: ActivePrompt,
    pub(super) todo: TodoState,
    /// With the direct tool loadout.
    pub(super) code_mode: Arc<crate::tools::code_mode::CodeModeSurface>,
    pub(super) added_dirs: AddedDirs,
    pub(super) left_out: LeftOut,
    pub(super) auth: String,
    /// The session's settings, once a host kept them.
    pub(super) settings: SettingsRecord,
}

impl DurableState {
    /// `snapshot` with this state in its metadata.
    pub(super) fn decorate(&self, snapshot: SessionSnapshot) -> SessionSnapshot {
        let Self {
            prompt,
            todo,
            code_mode,
            added_dirs,
            left_out,
            auth,
            settings,
        } = self;
        let snapshot = todo.decorate(prompt.decorate(code_mode.decorate(snapshot)));
        settings.decorate(super::agent::decorate_model_override(
            left_out.decorate(added_dirs.decorate(snapshot)),
            auth,
        ))
    }
}

/// The node a refused turn rolls back to: the active leaf, or for a session's
/// first turn a new session-start node holding the state before the prompt.
fn turn_base(
    stored: &StoredSession,
    snapshot: impl FnOnce() -> SessionSnapshot,
) -> anyhow::Result<NodeId> {
    let active_leaf = || stored.with_session_tree(|tree| Ok(tree.active_leaf_id().cloned()));
    if let Some(leaf) = active_leaf()? {
        return Ok(leaf);
    }
    stored.save_snapshot(&snapshot(), &[])?;
    active_leaf()?.ok_or_else(|| anyhow::anyhow!("session start was saved without a leaf"))
}

/// One turn's SDK checkpoint store, installed by
/// [`SessionRuntime::install_turn_checkpoint`].
#[derive(Clone)]
pub(super) struct TurnCheckpoint(Arc<CheckpointStore>);

impl std::fmt::Debug for TurnCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnCheckpoint").finish_non_exhaustive()
    }
}

impl TurnCheckpoint {
    /// Records the row shown for a steer the run may apply as `model`, so
    /// saves write `shown` in place of the stamped copy.
    pub(super) fn record_steer(&self, model: &UserInput, shown: &UserInput) {
        self.0.lock_display().steered.push((
            Message::User(model.blocks().to_vec()),
            Message::User(shown.blocks().to_vec()),
        ));
    }

    /// Records the row shown for a background result the run applied as
    /// `input`, so saves write it in place of the model's copy.
    pub(super) fn record_display(&self, input: &UserInput, shown: Message) {
        self.0
            .lock_display()
            .applied
            .push_back((Message::BoundaryInput(input.blocks().to_vec()), shown));
    }
}

/// What a refusal of one turn rolls back to.
enum Rollback {
    /// `stored`'s node before the turn; see [`turn_base`].
    ToNode { stored: StoredSession, base: NodeId },
    /// The live history before the prompt, for an ephemeral session.
    LiveHistory,
    /// Nothing: a continuation added no prompt.
    Never,
}

struct CheckpointStore {
    /// The session file, or `None` for an ephemeral session.
    stored: Option<StoredSession>,
    rollback: Rollback,
    /// The session state as the turn began; only the host changes it, such
    /// as its settings, between turns.
    state: DurableState,
    display: Mutex<TurnDisplay>,
}

impl CheckpointStore {
    fn lock_display(&self) -> std::sync::MutexGuard<'_, TurnDisplay> {
        self.display
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Saves `snapshot` to the session file.
    fn save_now(&self, snapshot: SessionSnapshot) -> anyhow::Result<()> {
        let snapshot = self.state.decorate(snapshot);
        if let Some(stored) = &self.stored {
            let mut display = self.lock_display();
            let pending = display.pending_rows(snapshot.history(), SaveScope::Step);
            stored.save_snapshot(&snapshot, &pending.rows)?;
            display.mark_written(pending);
        }
        Ok(())
    }
}

impl SessionStore for CheckpointStore {
    fn load<'a>(&'a self, id: &'a SessionId) -> SessionStoreFuture<'a, Option<SessionSnapshot>> {
        match &self.stored {
            Some(stored) => SessionStore::load(stored, id),
            None => Box::pin(async { Ok(None) }),
        }
    }

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            self.save_now(snapshot)
                .map_err(|error| rho_sdk::Error::Persistence {
                    message: format!("{error:#}"),
                    source: None,
                })
        })
    }
}

/// The display rows one turn has and has not written.
///
/// History from `history_start` on is this turn: the model's copy of the
/// user's message, then everything the run appends. The run pushes that copy
/// before its first save. Display shows the user's message as sent in its
/// place, then the appended rows as they are, except that background results
/// show the rows recorded for them.
/// A compaction can rewrite history before or during the turn, so once the
/// model's copy is no longer at `history_start`, appended rows are lost and
/// the turn falls back to its recorded results and final reply.
#[derive(Debug)]
struct TurnDisplay {
    /// The user's message as sent, until a save writes it.
    unsaved_user: Option<Message>,
    /// The model's copy of the user's message.
    input: Message,
    history_start: usize,
    followed: Followed,
    /// Background results applied and not yet written: the model's copy
    /// and the row shown for it, in the order the run applied them.
    applied: VecDeque<(Message, Message)>,
    /// Steers offered to the run: the model's stamped copy and the row shown
    /// for it. A withdrawn steer never matches a history row.
    steered: Vec<(Message, Message)>,
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
    fn new(user: Message, input: Message, history_start: usize) -> Self {
        Self {
            unsaved_user: Some(user),
            input,
            history_start,
            followed: Followed::Tracking { written: 0 },
            applied: VecDeque::new(),
            steered: Vec::new(),
        }
    }

    /// Rows a save of `history` must add to the display transcript.
    fn pending_rows(&self, history: &[Message], scope: SaveScope) -> PendingRows {
        let mut rows: Vec<Message> = self.unsaved_user.iter().cloned().collect();
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
            } else if scope == SaveScope::Step
                && matches!(
                    row.semantic(),
                    SemanticMessage::User(_) | SemanticMessage::BoundaryInput(_)
                )
            {
                // Its display row may not be recorded yet; a later save
                // writes it.
                break;
            } else {
                let shown = self
                    .steered
                    .iter()
                    .find_map(|(model, shown)| (model == row).then_some(shown));
                rows.push(shown.unwrap_or(row).clone());
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

    /// A turn that continues `history` whose last row, `input`, sits at
    /// `history_start`: there is no user message to write, and the rows
    /// the run appends after `input` are the turn's.
    fn continuing(input: Message, history_start: usize) -> Self {
        Self {
            unsaved_user: None,
            input,
            history_start,
            followed: Followed::Tracking { written: 0 },
            applied: VecDeque::new(),
            steered: Vec::new(),
        }
    }

    /// Records that the user's message was saved ahead of the turn's rows.
    fn mark_user_written(&mut self) {
        self.unsaved_user = None;
    }

    /// Records that a save wrote `pending`'s rows.
    fn mark_written(&mut self, pending: PendingRows) {
        self.unsaved_user = None;
        self.applied.drain(..pending.applied_written);
        self.followed = pending.followed;
    }

    /// Where this turn's input sits in `history` when everything after it is
    /// model replies without tool calls; `None` once the turn ran tools, took
    /// steering or background input, or was compacted.
    fn prompt_only_start(&self, history: &[Message]) -> Option<usize> {
        let start = self.history_start;
        if history.get(start) != Some(&self.input) {
            return None;
        }
        history[start + 1..]
            .iter()
            .all(|row| {
                row.completed_assistant_content().is_some_and(|content| {
                    !content
                        .iter()
                        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
                })
            })
            .then_some(start)
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
#[path = "persist_tests.rs"]
mod tests;
