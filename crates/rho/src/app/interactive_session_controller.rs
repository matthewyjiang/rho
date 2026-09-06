use rho_sdk::{
    model::{handoff::HandoffReport, Message},
    RunOutcome, Session, SessionId,
};

use crate::{
    session::Session as StoredSession,
    tools::{advisor::AdvisorSessionStore, web::WebAccessStore},
};

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
    pending_omission: Option<HandoffReport>,
    persisted_turn_display: usize,
    web_access: WebAccessStore,
    advisor: Option<AdvisorSessionStore>,
}

impl InteractiveSessionController {
    pub(crate) fn new(
        session: Session,
        storage: Option<StoredSession>,
        web_access: WebAccessStore,
        advisor: Option<AdvisorSessionStore>,
    ) -> Self {
        let controller = Self {
            session,
            storage,
            pending_session_id: None,
            pending_omission: None,
            persisted_turn_display: 0,
            web_access,
            advisor,
        };
        controller.sync_web_access();
        controller.sync_advisor_session();
        controller
    }

    fn sync_web_access(&self) {
        let root = self.storage.as_ref().and_then(StoredSession::web_dir);
        self.web_access.bind_session(root);
    }

    /// Points the advisor at the session now in use. Every session replacement
    /// runs through here, so the advisor never reads a retired session.
    fn sync_advisor_session(&self) {
        if let Some(advisor) = &self.advisor {
            advisor.bind_session(self.session.clone());
        }
    }

    pub(crate) fn session(&self) -> &Session {
        &self.session
    }

    pub(crate) fn replace_session(&mut self, session: Session, omission: Option<HandoffReport>) {
        self.session = session;
        self.pending_session_id = None;
        self.persisted_turn_display = 0;
        self.pending_omission = omission.filter(HandoffReport::has_omissions);
        self.sync_advisor_session();
    }

    /// Replaces only the SDK session used by the current runtime policy.
    ///
    /// Pending durable-session identity and storage state must survive policy
    /// rebuilds until the next turn realizes the replacement.
    pub(crate) fn replace_runtime_session(&mut self, session: Session) {
        self.session = session;
        self.sync_advisor_session();
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
        self.persisted_turn_display = 0;
        self.sync_web_access();
    }

    pub(crate) fn storage(&self) -> Option<&StoredSession> {
        self.storage.as_ref()
    }

    pub(crate) fn take_pending_omission(&mut self) -> Option<HandoffReport> {
        self.pending_omission.take()
    }

    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        self.take_pending_omission()
            .map(|report| {
                format!(
                    "omitted {} incompatible provider-native context block(s) while resuming session (kinds: {})",
                    report.omitted_provider_context,
                    report.omitted_kinds.join(", ")
                )
            })
            .into_iter()
            .collect()
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
        self.sync_web_access();
        self.persisted_turn_display = 0;
        let session_id = SessionId::new();
        self.pending_session_id = Some(session_id.clone());
        Ok(session_id)
    }

    pub(crate) fn set_resumed_storage(&mut self, storage: StoredSession) {
        self.storage = Some(storage);
        self.persisted_turn_display = 0;
        self.sync_web_access();
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
        let display =
            pending_turn.map_or_else(Vec::new, |turn| turn.display_tail(&history, outcome));
        let display_tail = display.get(self.persisted_turn_display..).ok_or_else(|| {
            anyhow::anyhow!(
                "turn display checkpoint exceeds accumulated history: persisted {}, accumulated {}",
                self.persisted_turn_display,
                display.len()
            )
        })?;
        storage.save_snapshot(&self.session.snapshot(), display_tail)?;
        self.persisted_turn_display = 0;
        Ok(())
    }

    pub(crate) fn save_automatic_compaction(
        &mut self,
        snapshot: &rho_sdk::SessionSnapshot,
        display: &[Message],
        outcome: &rho_sdk::CompactionOutcome,
    ) -> anyhow::Result<()> {
        if let Some(storage) = &self.storage {
            let display_tail = display.get(self.persisted_turn_display..).ok_or_else(|| {
                anyhow::anyhow!("compaction display checkpoint exceeds accumulated history: persisted {}, accumulated {}", self.persisted_turn_display, display.len())
            })?;
            storage.save_compaction_snapshot(snapshot, display_tail, outcome)?;
            self.persisted_turn_display = display.len();
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
