use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex, MutexGuard, RwLock,
    },
};

use crate::{
    client::Rho,
    event::RunOutcome,
    model::{ContentBlock, ImageContent, Message},
    orchestration::execute_run,
    provider::ModelProvider,
    run::Run,
    Error, Revision, RunId, SessionId,
};

/// Validated user input accepted by a session run.
#[derive(Clone, Debug, PartialEq)]
pub struct UserInput {
    content: Vec<ContentBlock>,
}

impl UserInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text(text.into())],
        }
    }

    pub fn content(content: Vec<ContentBlock>) -> Result<Self, Error> {
        if content.is_empty() {
            return Err(Error::InvalidConfiguration {
                message: "user input must contain at least one content block".into(),
            });
        }
        Ok(Self { content })
    }

    pub fn text_and_images(
        text: impl Into<String>,
        images: impl IntoIterator<Item = ImageContent>,
    ) -> Self {
        let mut content = vec![ContentBlock::Text(text.into())];
        content.extend(images.into_iter().map(ContentBlock::Image));
        Self { content }
    }

    pub fn blocks(&self) -> &[ContentBlock] {
        &self.content
    }

    pub(crate) fn into_blocks(self) -> Vec<ContentBlock> {
        self.content
    }
}

/// Explicit lifecycle state for a session's active run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionState {
    #[default]
    Idle,
    Running,
    WaitingForHostInput,
    Cancelling,
    Completed,
    Failed,
}

impl SessionState {
    const fn code(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Running => 1,
            Self::WaitingForHostInput => 2,
            Self::Cancelling => 3,
            Self::Completed => 4,
            Self::Failed => 5,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Idle,
            1 => Self::Running,
            2 => Self::WaitingForHostInput,
            3 => Self::Cancelling,
            4 => Self::Completed,
            5 => Self::Failed,
            _ => unreachable!("session state is written only through SessionState::code"),
        }
    }
}

#[derive(Debug)]
struct SessionData {
    history: Vec<Message>,
    revision: Revision,
    compaction: crate::CompactionState,
    metadata: BTreeMap<String, String>,
    prompt_cache_key: Option<String>,
}

pub(crate) struct SessionCore {
    id: SessionId,
    data: Mutex<SessionData>,
    runtime: RwLock<Rho>,
    approvals: Arc<crate::workspace::SessionApprovals>,
    // Run ownership remains active until its guard finalizes, independently of presentation state.
    active_run: Mutex<Option<RunId>>,
    state: AtomicU8,
}

impl SessionCore {
    pub(crate) fn new(
        id: SessionId,
        history: Vec<Message>,
        revision: Revision,
        compaction: crate::CompactionState,
        metadata: BTreeMap<String, String>,
        prompt_cache_key: Option<String>,
        runtime: Rho,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            data: Mutex::new(SessionData {
                history,
                revision,
                compaction,
                metadata,
                prompt_cache_key,
            }),
            runtime: RwLock::new(runtime),
            approvals: Arc::default(),
            active_run: Mutex::new(None),
            state: AtomicU8::new(SessionState::Idle.code()),
        })
    }

    pub(crate) fn runtime(&self) -> Rho {
        self.runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn approvals(&self) -> Arc<crate::workspace::SessionApprovals> {
        Arc::clone(&self.approvals)
    }

    pub(crate) fn metadata(&self) -> BTreeMap<String, String> {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .metadata
            .clone()
    }

    pub(crate) fn prompt_cache_key(&self) -> Option<String> {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .prompt_cache_key
            .clone()
    }

    pub(crate) fn snapshot(&self) -> (Vec<Message>, Revision) {
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (data.history.clone(), data.revision)
    }

    pub(crate) fn commit(&self, history: Vec<Message>) -> Result<Revision, Error> {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let revision = data
            .revision
            .checked_next()
            .ok_or_else(|| Error::Persistence {
                message: "session revision is exhausted".into(),
            })?;
        data.history = history;
        data.revision = revision;
        Ok(revision)
    }

    pub(crate) fn compaction_state(&self) -> crate::CompactionState {
        self.data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .compaction
            .clone()
    }

    pub(crate) fn commit_compaction(
        &self,
        history: Vec<Message>,
    ) -> Result<crate::CompactionOutcome, Error> {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let revision = data
            .revision
            .checked_next()
            .ok_or_else(|| Error::Persistence {
                message: "session revision is exhausted".into(),
            })?;
        let previous_messages = data.history.len();
        let current_messages = history.len();
        data.compaction
            .record(previous_messages.saturating_sub(current_messages), revision);
        data.history = history;
        data.revision = revision;
        Ok(crate::CompactionOutcome::new(
            previous_messages,
            current_messages,
            revision,
        ))
    }

    pub(crate) fn set_state(&self, state: SessionState) {
        self.state.store(state.code(), Ordering::Release);
    }

    pub(crate) fn state(&self) -> SessionState {
        SessionState::from_code(self.state.load(Ordering::Acquire))
    }

    pub(crate) fn begin_run(&self, run_id: &RunId) -> Result<(), Error> {
        let mut active_run = self.lock_inactive()?;
        *active_run = Some(run_id.clone());
        self.set_state(SessionState::Running);
        Ok(())
    }

    fn lock_inactive(&self) -> Result<MutexGuard<'_, Option<RunId>>, Error> {
        let active_run = self
            .active_run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active_run.is_some() {
            Err(Error::SessionBusy)
        } else {
            Ok(active_run)
        }
    }

    pub(crate) fn finish_run(&self, run_id: &RunId) {
        let mut active_run = self
            .active_run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active_run.as_ref() != Some(run_id) {
            return;
        }
        *active_run = None;
        if !matches!(self.state(), SessionState::Completed | SessionState::Failed) {
            self.set_state(SessionState::Idle);
        }
    }

    fn has_active_run(&self) -> bool {
        self.active_run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }
}

pub(crate) struct ActiveRunGuard {
    core: Arc<SessionCore>,
    run_id: RunId,
    lifecycle: Arc<crate::client::RuntimeLifecycle>,
}

impl ActiveRunGuard {
    pub(crate) fn new(
        core: Arc<SessionCore>,
        run_id: RunId,
        lifecycle: Arc<crate::client::RuntimeLifecycle>,
    ) -> Self {
        Self {
            core,
            run_id,
            lifecycle,
        }
    }
}

impl Drop for ActiveRunGuard {
    fn drop(&mut self) {
        self.lifecycle.unregister(&self.run_id);
        self.core.finish_run(&self.run_id);
    }
}

/// Mutable conversation state owned by one SDK runtime.
#[derive(Clone)]
pub struct Session {
    core: Arc<SessionCore>,
}

impl Session {
    pub(crate) fn from_core(core: Arc<SessionCore>) -> Self {
        Self { core }
    }

    pub fn id(&self) -> &SessionId {
        &self.core.id
    }

    pub fn revision(&self) -> Revision {
        self.core.snapshot().1
    }

    pub fn history(&self) -> Vec<Message> {
        self.core.snapshot().0
    }

    pub fn snapshot(&self) -> crate::SessionSnapshot {
        let (history, revision) = self.core.snapshot();
        let mut snapshot = crate::SessionSnapshot::new(
            self.id().clone(),
            revision,
            history,
            self.core.runtime().provider.identity(),
            self.core.compaction_state(),
        );
        for (key, value) in self.core.metadata() {
            snapshot = snapshot.with_metadata(key, value);
        }
        if let Some(prompt_cache_key) = self.core.prompt_cache_key() {
            snapshot = snapshot.with_prompt_cache_key(prompt_cache_key);
        }
        snapshot
    }

    pub fn reasoning_level(&self) -> crate::ReasoningLevel {
        self.core.runtime().reasoning_level
    }

    pub fn set_reasoning_level(&self, reasoning_level: crate::ReasoningLevel) -> Result<(), Error> {
        let _inactive = self.core.lock_inactive()?;
        self.core
            .runtime
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reasoning_level = reasoning_level;
        Ok(())
    }

    pub fn diagnostics(&self) -> crate::DiagnosticsSnapshot {
        self.core.runtime().diagnostics()
    }

    pub fn state(&self) -> SessionState {
        self.core.state()
    }

    pub fn is_running(&self) -> bool {
        self.core.has_active_run()
    }

    pub async fn start(&self, input: UserInput) -> Result<Run, Error> {
        let run_id = RunId::new();
        self.core.begin_run(&run_id)?;

        let cancellation = crate::CancellationToken::new();
        let runtime = self.core.runtime();
        let core = Arc::clone(&self.core);
        let (events, receiver) = tokio::sync::mpsc::channel(runtime.event_capacity.get());
        let (commands, command_receiver) = tokio::sync::mpsc::channel(runtime.event_capacity.get());
        let worker_cancellation = cancellation.clone();
        let worker_run_id = run_id.clone();
        let guard = ActiveRunGuard::new(
            Arc::clone(&core),
            run_id.clone(),
            Arc::clone(&runtime.lifecycle),
        );
        runtime
            .lifecycle
            .register(run_id.clone(), cancellation.clone())?;
        let worker = tokio::spawn(async move {
            let _guard = guard;
            execute_run(
                core,
                runtime,
                worker_run_id,
                input,
                worker_cancellation,
                events,
                command_receiver,
            )
            .await
        });
        Ok(Run::new(run_id, cancellation, receiver, commands, worker))
    }

    pub async fn complete(&self, input: impl Into<String>) -> Result<RunOutcome, Error> {
        let mut run = self.start(UserInput::text(input)).await?;
        while let Some(event) = run.next_event().await {
            if let crate::RunEvent::HostInputRequested { request } = event {
                run.cancel();
                let _ = run.outcome().await;
                return Err(Error::InvalidHostResponse {
                    message: format!(
                        "simple completion cannot answer host input request '{}'",
                        request.id()
                    ),
                });
            }
        }
        run.outcome().await
    }

    pub async fn compact(&self) -> Result<crate::CompactionOutcome, Error> {
        let runtime = self.core.runtime();
        let compactor =
            runtime
                .compactor
                .as_ref()
                .cloned()
                .ok_or_else(|| Error::InvalidConfiguration {
                    message: "no compactor is configured".into(),
                })?;
        let run_id = RunId::new();
        self.core.begin_run(&run_id)?;
        let cancellation = crate::CancellationToken::new();
        let _guard = ActiveRunGuard::new(
            Arc::clone(&self.core),
            run_id.clone(),
            Arc::clone(&runtime.lifecycle),
        );
        runtime.lifecycle.register(run_id, cancellation.clone())?;
        let history = self.history();
        let output = compactor
            .compact(crate::CompactionRequest::new(history, cancellation))
            .await?;
        let outcome = self.core.commit_compaction(output.into_messages())?;
        self.core.set_state(SessionState::Completed);
        Ok(outcome)
    }

    /// Appends host-provided context while the session is idle.
    pub fn append_message(&self, message: Message) -> Result<Revision, Error> {
        let _inactive = self.core.lock_inactive()?;
        let mut history = self.history();
        history.push(message);
        self.core.commit(history)
    }

    pub fn reset(&self) -> Result<(), Error> {
        let _inactive = self.core.lock_inactive()?;
        let system_prompt = match &self.core.runtime().system_prompt {
            crate::SystemPrompt::Custom(prompt) => Some(Message::System(prompt.clone())),
            crate::SystemPrompt::None => None,
        };
        let mut data = self
            .core
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.history = system_prompt.into_iter().collect();
        data.compaction = crate::CompactionState::default();
        data.revision = data
            .revision
            .checked_next()
            .ok_or_else(|| Error::Persistence {
                message: "session revision is exhausted".into(),
            })?;
        Ok(())
    }

    pub fn replace_provider(
        &self,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<crate::model::handoff::HandoffReport, Error> {
        let _inactive = self.core.lock_inactive()?;
        let report =
            crate::model::handoff::report_message_omissions(&self.history(), &provider.identity());
        self.core
            .runtime
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .provider = provider;
        Ok(report)
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("id", self.id())
            .field("revision", &self.revision())
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}
