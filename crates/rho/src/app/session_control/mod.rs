//! The session-control core: what an agent may do to the daemon's sessions,
//! under one policy, whichever front end asks. `rho mcp serve`
//! (`app::mcp_server`) is a thin adapter over it, and the native `sessions`
//! tool's control actions are meant to be another (#1714).
//!
//! [`SessionControl`] serves seven actions ([`Action`]): `list`, `read`,
//! `new`, `prompt`, `steer`, `cancel`, and `wait`. Each call carries the
//! caller's [`Grant`], and the core enforces it: the access the action
//! needs, the workspaces the grant reaches, the nesting depth
//! ([`Limits::max_depth`]), and how many sessions each holder may have
//! running ([`Limits::max_sessions`]). Every refusal names its budget, its
//! limit, and the value asked for ([`Refusal`]).
//!
//! No action answers a session's approvals or questions, and the core never
//! sends `rho/session/answer` or `rho/session/hold`: a turn that asks waits
//! for a person, and `wait` reports it waiting for input.
//!
//! The daemon is reached as any client reaches it, starting it when none
//! runs. Requests share one connection; `read` and `wait` observe on a
//! connection of their own, so calls on the same session never replace each
//! other's attachment. Observing never loads a session, so reading one
//! never starts its MCP servers at the reader's nesting depth.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use rho_protocol::{
    methods::{
        JobList, SessionCancel, SessionConfigure, SessionList, SessionNew, SessionPrompt,
        SessionSteer,
    },
    ClientEnvironment, ClientError, ContentBlock, JobKind, JobListParams, JobState, JobSummary,
    ModelRef, SessionCancelParams, SessionChange, SessionConfigureParams, SessionId,
    SessionListParams, SessionNewParams, SessionPromptParams, SessionSetup, SessionSteerParams,
};

use super::{
    daemon_cli::Unreachable,
    daemon_client::{Daemon, DaemonError},
};

mod dispatch;
mod follow;
mod params;
mod policy;
mod transcript;

use params::{
    CancelParams, CancelResult, ListParams, ListResult, NewParams, NewResult, PromptParams,
    PromptResult, ReadParams, ReadResult, SessionEntry, SteerParams, SteerResult, WaitParams,
    WaitResult,
};
pub(crate) use policy::{
    Access, Action, Grant, Holder, Limits, Refusal, Workspaces, SESSION_DEPTH_ENV,
};

/// Why an action failed. Its `Display` is what the caller sees.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ControlError {
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// The session is neither one the daemon has a job for nor a saved one.
    #[error("no session {0} is known to the rho daemon")]
    UnknownSession(SessionId),
    /// The call's arguments are not ones the action takes.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Daemon(#[from] DaemonError),
}

impl From<ClientError> for ControlError {
    fn from(error: ClientError) -> Self {
        Self::Daemon(error.into())
    }
}

impl From<Unreachable> for ControlError {
    fn from(error: Unreachable) -> Self {
        Self::Daemon(error.into())
    }
}

/// Where the calling process sits: the nesting depth of the session that
/// started it, 0 for a person's, and the environment its sessions run with.
pub(crate) struct Caller {
    pub(crate) depth: u32,
    pub(crate) environment: ClientEnvironment,
}

/// The core. One per process; every grant shares it.
pub(crate) struct SessionControl {
    daemon: Daemon,
    caller: Caller,
    limits: Limits,
    /// The sessions each holder started or prompted, which its session cap
    /// counts while their turns run.
    driven: Mutex<HashMap<Holder, HashSet<SessionId>>>,
    /// Held from a cap's check until the turn it admits started, so two
    /// calls cannot both take the last slot.
    admission: tokio::sync::Mutex<()>,
}

impl SessionControl {
    pub(crate) fn new(caller: Caller, limits: Limits) -> Self {
        Self {
            daemon: Daemon::default(),
            caller,
            limits,
            driven: Mutex::default(),
            admission: tokio::sync::Mutex::default(),
        }
    }

    /// The daemon's sessions the grant reaches, the most recently changed
    /// first.
    pub(crate) async fn list(
        &self,
        grant: &Grant,
        params: ListParams,
    ) -> Result<ListResult, ControlError> {
        grant.permit(Action::List)?;
        let limit = params.limit()?;
        let workspace = params.workspace.as_deref().map(canonical).transpose()?;
        let mut sessions: Vec<SessionEntry> = self
            .jobs()
            .await?
            .into_iter()
            .filter(|job| job.kind == JobKind::Session)
            .filter(|job| grant.workspaces.allows(&job.workspace))
            .filter(|job| workspace.as_ref().is_none_or(|only| &job.workspace == only))
            .filter(|job| params.state.is_none_or(|state| job.state == state))
            .filter_map(SessionEntry::of_job)
            .collect();
        sessions.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
        let more = sessions.len() > limit;
        sessions.truncate(limit);
        Ok(ListResult { sessions, more })
    }

    /// A window of the session's transcript as text, its turn in progress
    /// included.
    pub(crate) async fn read(
        &self,
        grant: &Grant,
        params: ReadParams,
    ) -> Result<ReadResult, ControlError> {
        grant.permit(Action::Read)?;
        self.reach_session(grant, &params.session).await?;
        let window = params.window()?;
        let text = follow::transcript(&params.session).await?;
        Ok(ReadResult::window(params.session, &text, window))
    }

    /// Opens a session in `workspace` and starts its first turn.
    pub(crate) async fn new_session(
        &self,
        grant: &Grant,
        params: NewParams,
    ) -> Result<NewResult, ControlError> {
        grant.permit(Action::New)?;
        let workspace = canonical(&params.workspace)?;
        grant.reach(&workspace)?;
        let content = prompt_content(params.prompt)?;
        let _admitted = self.admit(grant, None).await?;
        let opened = self
            .daemon
            .request::<SessionNew>(&SessionNewParams {
                cwd: workspace.clone(),
                environment: self.child_environment(),
                agent: params.agent,
                setup: SessionSetup::default(),
            })
            .await?;
        if let Some(model) = params.model {
            let model = choose_model(&opened.options.model.available, &model)?;
            self.daemon
                .request::<SessionConfigure>(&SessionConfigureParams {
                    session: opened.session.clone(),
                    change: SessionChange::Model { model },
                })
                .await?;
        }
        self.start_turn(grant, &opened.session, content).await?;
        Ok(NewResult {
            session: opened.session,
            workspace,
        })
    }

    /// Starts a turn on a session that has none in progress.
    pub(crate) async fn prompt(
        &self,
        grant: &Grant,
        params: PromptParams,
    ) -> Result<PromptResult, ControlError> {
        grant.permit(Action::Prompt)?;
        self.reach_session(grant, &params.session).await?;
        let content = prompt_content(params.text)?;
        let _admitted = self.admit(grant, Some(&params.session)).await?;
        self.start_turn(grant, &params.session, content).await?;
        Ok(PromptResult {
            session: params.session,
        })
    }

    /// Stages text for the running turn's next model request.
    pub(crate) async fn steer(
        &self,
        grant: &Grant,
        params: SteerParams,
    ) -> Result<SteerResult, ControlError> {
        grant.permit(Action::Steer)?;
        self.reach_session(grant, &params.session).await?;
        let steered = self
            .daemon
            .request::<SessionSteer>(&SessionSteerParams {
                session: params.session,
                text: params.text,
            })
            .await?;
        Ok(SteerResult { id: steered.id })
    }

    /// Cancels the session's turn in progress, if any.
    pub(crate) async fn cancel(
        &self,
        grant: &Grant,
        params: CancelParams,
    ) -> Result<CancelResult, ControlError> {
        grant.permit(Action::Cancel)?;
        self.reach_session(grant, &params.session).await?;
        self.daemon
            .request::<SessionCancel>(&SessionCancelParams {
                session: params.session,
            })
            .await?;
        Ok(CancelResult {})
    }

    /// Waits up to the call's timeout for the session's turn to end or ask
    /// for input, then reports where it is, with a cursor the next `wait`
    /// resumes from.
    pub(crate) async fn wait(
        &self,
        grant: &Grant,
        params: WaitParams,
    ) -> Result<WaitResult, ControlError> {
        grant.permit(Action::Wait)?;
        self.reach_session(grant, &params.session).await?;
        let timeout = params.timeout()?;
        let after = params.cursor()?;
        follow::wait(params.session, after, timeout).await
    }

    /// Prompts `session` and counts it toward the holder's session cap.
    async fn start_turn(
        &self,
        grant: &Grant,
        session: &SessionId,
        content: Vec<ContentBlock>,
    ) -> Result<(), ControlError> {
        self.daemon
            .request::<SessionPrompt>(&SessionPromptParams {
                session: session.clone(),
                content,
                timeout_ms: None,
                environment: self.child_environment(),
            })
            .await?;
        self.driven
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(grant.holder.clone())
            .or_default()
            .insert(session.clone());
        Ok(())
    }

    /// Checks the depth and session caps for a turn the holder starts on
    /// `session`, or on a new one. The guard admits the turn until dropped.
    async fn admit(
        &self,
        grant: &Grant,
        session: Option<&SessionId>,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, ControlError> {
        let asked = self.caller.depth.saturating_add(1);
        if asked > self.limits.max_depth {
            return Err(Refusal::Depth {
                limit: self.limits.max_depth,
                asked,
            }
            .into());
        }
        let admitted = self.admission.lock().await;
        let jobs = self.jobs().await?;
        let running: HashSet<&SessionId> = jobs
            .iter()
            .filter(|job| in_progress(job.state))
            .filter_map(|job| job.session.as_ref())
            .collect();
        let mut driven = self.driven.lock().unwrap_or_else(PoisonError::into_inner);
        let mine = driven.entry(grant.holder.clone()).or_default();
        mine.retain(|session| running.contains(session));
        // A session whose turn already runs starts no new one: the daemon
        // refuses that prompt itself.
        let adds = session.is_none_or(|session| !mine.contains(session));
        let asked = u32::try_from(mine.len())
            .unwrap_or(u32::MAX)
            .saturating_add(u32::from(adds));
        if adds && asked > self.limits.max_sessions {
            return Err(Refusal::Sessions {
                holder: grant.holder.clone(),
                limit: self.limits.max_sessions,
                asked,
            }
            .into());
        }
        Ok(admitted)
    }

    /// Refuses a session outside the grant's workspaces. A grant that
    /// reaches every workspace needs no lookup.
    async fn reach_session(&self, grant: &Grant, session: &SessionId) -> Result<(), ControlError> {
        if grant.workspaces == Workspaces::Any {
            return Ok(());
        }
        let workspace = self.workspace_of(session).await?;
        Ok(grant.reach(&workspace)?)
    }

    /// The workspace of `session`: its job's, or, for a saved session no
    /// daemon has run, the catalog's.
    async fn workspace_of(&self, session: &SessionId) -> Result<PathBuf, ControlError> {
        let jobs = self.jobs().await?;
        if let Some(job) = jobs
            .iter()
            .find(|job| job.session.as_ref() == Some(session))
        {
            return Ok(job.workspace.clone());
        }
        let mut cursor = None;
        loop {
            let page = self
                .daemon
                .request::<SessionList>(&SessionListParams {
                    cwd: None,
                    cursor,
                    limit: std::num::NonZeroU32::new(CATALOG_PAGE),
                })
                .await?;
            if let Some(saved) = page.sessions.iter().find(|saved| &saved.session == session) {
                return Ok(saved.cwd.clone());
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Err(ControlError::UnknownSession(session.clone())),
            }
        }
    }

    async fn jobs(&self) -> Result<Vec<JobSummary>, ControlError> {
        Ok(self
            .daemon
            .request::<JobList>(&JobListParams {})
            .await?
            .jobs)
    }

    /// The environment the sessions this process starts or prompts run
    /// with: its own, one level deeper.
    fn child_environment(&self) -> ClientEnvironment {
        let depth = self.caller.depth.saturating_add(1).to_string();
        self.caller
            .environment
            .iter()
            .filter(|(name, _)| *name != SESSION_DEPTH_ENV)
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .chain([(SESSION_DEPTH_ENV.to_owned(), depth)])
            .collect()
    }
}

/// The catalog page `workspace_of` walks: the daemon's largest
/// (`rho/session/list` refuses more than 1000).
const CATALOG_PAGE: u32 = 1000;

/// Whether a turn is under way in this state, so the session counts toward
/// its holder's cap and `wait` keeps waiting.
fn in_progress(state: JobState) -> bool {
    matches!(
        state,
        JobState::Running | JobState::Queued | JobState::WaitingForInput
    )
}

/// `path` resolved through symlinks, as the daemon records workspaces.
fn canonical(path: &Path) -> Result<PathBuf, ControlError> {
    if !path.is_absolute() {
        return Err(ControlError::Invalid(format!(
            "workspace {} is not an absolute path",
            path.display()
        )));
    }
    match path.canonicalize() {
        Ok(resolved) if resolved.is_dir() => Ok(resolved),
        Ok(_) => Err(ControlError::Invalid(format!(
            "workspace {} is not a directory",
            path.display()
        ))),
        Err(error) => Err(ControlError::Invalid(format!(
            "workspace {}: {error}",
            path.display()
        ))),
    }
}

fn prompt_content(text: String) -> Result<Vec<ContentBlock>, ControlError> {
    if text.trim().is_empty() {
        return Err(ControlError::Invalid("the prompt is empty".into()));
    }
    Ok(vec![ContentBlock::Text { text }])
}

/// The model `wanted` names among those the session offers: `provider/model`,
/// or a model id one provider alone offers.
fn choose_model(
    available: &[rho_protocol::ModelChoice],
    wanted: &str,
) -> Result<ModelRef, ControlError> {
    let named = |choice: &&rho_protocol::ModelChoice| {
        format!("{}/{}", choice.model.provider, choice.model.model) == wanted
    };
    if let Some(choice) = available.iter().find(named) {
        return Ok(choice.model.clone());
    }
    let by_id: Vec<_> = available
        .iter()
        .filter(|choice| choice.model.model == wanted)
        .collect();
    match by_id.as_slice() {
        [choice] => Ok(choice.model.clone()),
        _ => {
            let offered: Vec<String> = available
                .iter()
                .map(|choice| format!("{}/{}", choice.model.provider, choice.model.model))
                .collect();
            Err(ControlError::Invalid(format!(
                "model {wanted:?} does not name one model the session offers; name it as provider/model, one of: {}",
                offered.join(", ")
            )))
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
