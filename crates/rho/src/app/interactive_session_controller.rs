use rho_sdk::{model::Message, RunOutcome, Session, SessionId};

use crate::session::Session as StoredSession;

use super::interactive_run_controller::PendingTurn;

pub(crate) enum ReplacementSessionSource {
    History {
        history: Vec<Message>,
        id: Option<String>,
    },
    DurableSnapshot {
        snapshot: rho_sdk::SessionSnapshot,
    },
    Snapshot {
        storage: StoredSession,
        id: String,
    },
}

pub(crate) struct InteractiveSessionController {
    session: Session,
    storage: Option<StoredSession>,
    pending_session_id: Option<SessionId>,
    pending_notices: Vec<String>,
    persisted_pending_user: bool,
}

impl InteractiveSessionController {
    pub(crate) fn new(session: Session, storage: Option<StoredSession>) -> Self {
        Self {
            session,
            storage,
            pending_session_id: None,
            pending_notices: Vec::new(),
            persisted_pending_user: false,
        }
    }

    pub(crate) fn session(&self) -> &Session {
        &self.session
    }

    pub(crate) fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub(crate) fn replace_session(&mut self, session: Session, notice: Option<String>) {
        self.session = session;
        self.pending_session_id = None;
        self.persisted_pending_user = false;
        if let Some(notice) = notice {
            self.pending_notices.push(notice);
        }
    }

    /// Replaces only the SDK session used by the current runtime policy.
    ///
    /// Pending durable-session identity and storage state must survive policy
    /// rebuilds until the next turn realizes the replacement.
    pub(crate) fn replace_runtime_session(&mut self, session: Session) {
        self.session = session;
    }

    pub(crate) fn history(&self) -> Vec<Message> {
        self.session.history()
    }

    pub(crate) fn id(&self) -> &SessionId {
        self.pending_session_id
            .as_ref()
            .unwrap_or_else(|| self.session.id())
    }

    pub(crate) fn attach_storage(&mut self, storage: StoredSession) {
        self.storage = Some(storage);
        self.persisted_pending_user = false;
    }

    pub(crate) fn storage(&self) -> Option<&StoredSession> {
        self.storage.as_ref()
    }

    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_notices)
    }

    pub(crate) fn pending_replacement(&self) -> Option<ReplacementSessionSource> {
        let id = self.pending_session_id.as_ref()?.to_string();
        Some(match &self.storage {
            Some(storage) => ReplacementSessionSource::Snapshot {
                storage: storage.clone(),
                id,
            },
            None => ReplacementSessionSource::History {
                history: Vec::new(),
                id: Some(id),
            },
        })
    }

    pub(crate) fn reset(&mut self) -> anyhow::Result<SessionId> {
        self.session.reset()?;
        self.storage = None;
        self.persisted_pending_user = false;
        let session_id = SessionId::new();
        self.pending_session_id = Some(session_id.clone());
        Ok(session_id)
    }

    pub(crate) fn set_resumed_storage(&mut self, storage: StoredSession) {
        self.storage = Some(storage);
        self.persisted_pending_user = false;
    }

    pub(crate) fn sync_finished_turn(
        &mut self,
        pending_turn: Option<&PendingTurn>,
        outcome: Option<&RunOutcome>,
    ) -> anyhow::Result<()> {
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        let history = self.session.history();
        let history_start = pending_turn.map_or(history.len(), PendingTurn::history_start);
        let current_turn_committed =
            pending_turn.is_some_and(|turn| history.get(history_start) == Some(turn.model_user()));
        let mut display_tail = if current_turn_committed {
            history[history_start..].to_vec()
        } else {
            pending_turn
                .map(|turn| turn.model_user().clone())
                .into_iter()
                .chain(outcome.and_then(|outcome| {
                    (!outcome.text().is_empty())
                        .then(|| Message::assistant_text(outcome.text().to_string()))
                }))
                .collect()
        };
        if let (Some(display), Some(first)) = (
            pending_turn.and_then(PendingTurn::display_user),
            display_tail.first_mut(),
        ) {
            *first = display.clone();
        }
        if self.persisted_pending_user && !display_tail.is_empty() {
            display_tail.remove(0);
        }
        storage.save_snapshot(&self.session.snapshot(), &display_tail)?;
        self.persisted_pending_user = false;
        Ok(())
    }

    pub(crate) fn save_automatic_compaction(
        &mut self,
        snapshot: &rho_sdk::SessionSnapshot,
        display_user: Option<&Message>,
        outcome: &rho_sdk::CompactionOutcome,
    ) -> anyhow::Result<()> {
        if let Some(storage) = &self.storage {
            let display_tail = if self.persisted_pending_user {
                &[][..]
            } else {
                display_user.map(std::slice::from_ref).unwrap_or_default()
            };
            storage.save_compaction_snapshot(snapshot, display_tail, outcome)?;
            self.persisted_pending_user |= display_user.is_some();
        }
        Ok(())
    }

    pub(crate) fn save_compaction_snapshot(
        &self,
        display_tail: &[Message],
        outcome: &rho_sdk::CompactionOutcome,
    ) -> anyhow::Result<()> {
        if let Some(storage) = &self.storage {
            storage.save_compaction_snapshot(&self.session.snapshot(), display_tail, outcome)?;
        }
        Ok(())
    }

    pub(crate) fn save_snapshot(&self, display_tail: &[Message]) -> anyhow::Result<()> {
        if let Some(storage) = &self.storage {
            storage.save_snapshot(&self.session.snapshot(), display_tail)?;
        }
        Ok(())
    }
}
