//! The session-control core: what an agent may do to the daemon's sessions,
//! under one policy, whichever front end asks. `rho mcp serve`
//! (`app::mcp_server`) and the native `sessions` tool's control actions
//! (`tools::sessions::control`) are thin adapters over it (#1714).
//!
//! [`SessionControl`] serves seven actions ([`Action`]): `list`, `read`,
//! `new`, `prompt`, `steer`, `cancel`, and `wait`. Each call carries its
//! [`Caller`]: the transport's [`Grant`], narrowed by the scope of the
//! session that calls, and that session's depth (`crate::caller_context`).
//! The core enforces it: the access the action needs, the workspaces the
//! grant reaches, the nesting depth ([`Limits::max_depth`]), and how many
//! sessions each holder may have running ([`Limits::max_sessions`]). Every
//! refusal names its budget, its limit, and the value asked for
//! ([`Refusal`]). A session the core starts or prompts runs with the
//! caller's context one level deeper, so the limits follow it down: the core
//! names that context as the request's grant, and the daemon records it on
//! the session's job and writes the session's environment from it (#1780). A
//! session a caller starts from a session of its own, as the native
//! `sessions` tool's callers do, records the job writing that session as its
//! parent, and its turns wait for capacity in the daemon-wide agent budget
//! as subagent runs do (#1714).
//!
//! No action answers a session's approvals or questions: a turn that asks
//! waits for a person, and `wait` reports it waiting for input. The daemon
//! holds the core to that. Its connections declare an agent's role
//! ([`ConnectAs::Agent`]), which the daemon refuses `rho/session/answer`
//! and `rho/session/hold` from (#1780).
//!
//! The daemon is reached as [`Reach`] allows: `rho mcp serve` starts it
//! when none runs, as any client does, and the native tool reaches only one
//! already running. Requests share one connection; `read` and `wait` observe on a
//! connection of their own, so calls on the same session never replace each
//! other's attachment. Observing never loads a session, so reading one
//! never starts its MCP servers at the reader's nesting depth.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use rho_protocol::{
    methods::{
        JobList, SessionCancel, SessionConfigure, SessionList, SessionNew, SessionPrompt,
        SessionSteer,
    },
    ClientEnvironment, ClientError, ContentBlock, JobId, JobKind, JobListParams, JobState,
    JobSummary, ModelRef, SessionCancelParams, SessionChange, SessionConfigureParams, SessionId,
    SessionListParams, SessionNewParams, SessionPromptParams, SessionSetup, SessionSteerParams,
};
use serde_json::Value;

use super::{
    daemon_cli::Unreachable,
    daemon_client::{Daemon, DaemonError},
};
use crate::daemon::ConnectAs;

mod check;
mod dispatch;
mod follow;
mod params;
mod policy;
mod transcript;

pub(crate) use super::daemon_cli::Reach;
use check::CheckedCall;
pub(crate) use check::{Checked, Target};
use dispatch::json;
use params::{
    CancelParams, CancelResult, ListParams, ListResult, NewParams, NewResult, PromptParams,
    PromptResult, ReadParams, ReadResult, SessionEntry, SteerParams, SteerResult, WaitParams,
    WaitResult,
};
pub(crate) use policy::{Action, Caller, Grant, Holder, Limits, Refusal};

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

/// The core. One per process; every caller shares it.
pub(crate) struct SessionControl {
    daemon: Daemon,
    /// How `daemon`, and the connections `read` and `wait` open, reach it.
    reach: Reach,
    limits: Limits,
    /// The sessions each holder started or prompted, which its session cap
    /// counts while their turns run.
    driven: Mutex<HashMap<Holder, HashSet<SessionId>>>,
    /// Each holder's admission lock, held from its cap's check until the
    /// turn it admits started, so two of its calls cannot both take its
    /// last slot. Holders do not wait on each other.
    admission: Mutex<HashMap<Holder, Arc<tokio::sync::Mutex<()>>>>,
}

impl SessionControl {
    /// A core that reaches the daemon as `reach` allows.
    pub(crate) fn new(limits: Limits, reach: Reach) -> Self {
        Self {
            daemon: Daemon::reaching(reach, ConnectAs::Agent),
            reach,
            limits,
            driven: Mutex::default(),
            admission: Mutex::default(),
        }
    }

    /// Runs a checked call: admits a start or prompt under the per-holder
    /// session cap, then acts, and returns its result as JSON.
    pub(crate) async fn run(
        &self,
        caller: &Caller,
        checked: Checked,
    ) -> Result<Value, ControlError> {
        match checked.call {
            CheckedCall::List(params) => json(self.list(caller, params).await?),
            CheckedCall::Read(params) => json(self.read(params).await?),
            CheckedCall::New { params, workspace } => {
                json(self.new_session(caller, params, workspace).await?)
            }
            CheckedCall::Prompt(params) => json(self.prompt(caller, params).await?),
            CheckedCall::Steer(params) => json(self.steer(params).await?),
            CheckedCall::Cancel(params) => json(self.cancel(params).await?),
            CheckedCall::Wait(params) => json(self.wait(params).await?),
        }
    }

    /// The daemon's sessions the grant reaches, the most recently changed
    /// first.
    async fn list(&self, caller: &Caller, params: ListParams) -> Result<ListResult, ControlError> {
        let limit = params.limit()?;
        let workspace = params.workspace.as_deref().map(canonical).transpose()?;
        let jobs = self.jobs().await?;
        let writers: HashMap<JobId, SessionId> = jobs
            .iter()
            .filter_map(|job| Some((job.id.clone(), job.session.clone()?)))
            .collect();
        let mut sessions: Vec<SessionEntry> = jobs
            .into_iter()
            .filter(|job| job.kind == JobKind::Session)
            .filter(|job| caller.grant.scope.workspaces.allows(&job.workspace))
            .filter(|job| workspace.as_ref().is_none_or(|only| &job.workspace == only))
            .filter(|job| params.state.is_none_or(|state| job.state == state))
            .filter_map(|job| SessionEntry::of_job(job, &writers))
            .collect();
        sessions.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
        let more = sessions.len() > limit;
        sessions.truncate(limit);
        Ok(ListResult { sessions, more })
    }

    /// A window of the session's transcript as text, its turn in progress
    /// included.
    async fn read(&self, params: ReadParams) -> Result<ReadResult, ControlError> {
        let window = params.window()?;
        let text = follow::transcript(self.reach, &params.session).await?;
        Ok(ReadResult::window(params.session, &text, window))
    }

    /// Opens a session in `workspace`, the params' canonical, and starts
    /// its first turn.
    async fn new_session(
        &self,
        caller: &Caller,
        params: NewParams,
        workspace: PathBuf,
    ) -> Result<NewResult, ControlError> {
        let content = prompt_content(&params.prompt)?;
        let _admitted = self.admit(caller, None).await?;
        let opened = self
            .daemon
            .request::<SessionNew>(&SessionNewParams {
                cwd: workspace.clone(),
                environment: Self::child_environment(caller),
                agent: params.agent,
                setup: SessionSetup::default(),
                parent: caller.session.clone(),
                grant: Some(caller.child_context().into()),
            })
            .await?;
        if let Some(model) = params.model {
            let model = choose_model(&opened.options.model.available, &model)?;
            self.daemon
                .request::<SessionConfigure>(&SessionConfigureParams {
                    session: opened.session.clone(),
                    change: SessionChange::Model { model, auth: None },
                })
                .await?;
        }
        self.start_turn(caller, &opened.session, content).await?;
        Ok(NewResult {
            session: opened.session,
            workspace,
        })
    }

    /// Starts a turn on a session that has none in progress.
    async fn prompt(
        &self,
        caller: &Caller,
        params: PromptParams,
    ) -> Result<PromptResult, ControlError> {
        let content = prompt_content(&params.text)?;
        let _admitted = self.admit(caller, Some(&params.session)).await?;
        self.start_turn(caller, &params.session, content).await?;
        Ok(PromptResult {
            session: params.session,
        })
    }

    /// Stages text for the running turn's next model request.
    async fn steer(&self, params: SteerParams) -> Result<SteerResult, ControlError> {
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
    async fn cancel(&self, params: CancelParams) -> Result<CancelResult, ControlError> {
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
    async fn wait(&self, params: WaitParams) -> Result<WaitResult, ControlError> {
        let timeout = params.timeout()?;
        let after = params.cursor()?;
        follow::wait(self.reach, params.session, after, timeout).await
    }

    /// Prompts `session` and counts it toward the holder's session cap.
    async fn start_turn(
        &self,
        caller: &Caller,
        session: &SessionId,
        content: Vec<ContentBlock>,
    ) -> Result<(), ControlError> {
        self.daemon
            .request::<SessionPrompt>(&SessionPromptParams {
                session: session.clone(),
                content,
                timeout_ms: None,
                environment: Self::child_environment(caller),
                grant: Some(caller.child_context().into()),
                idempotency_key: None,
            })
            .await?;
        self.driven
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(caller.grant.holder.clone())
            .or_default()
            .insert(session.clone());
        Ok(())
    }

    /// Checks the session cap for a turn the holder starts on `session`, or
    /// on a new one. The guard admits the turn until dropped: the holder's
    /// other starts wait for it, so two cannot both take its last slot.
    async fn admit(
        &self,
        caller: &Caller,
        session: Option<&SessionId>,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, ControlError> {
        let holder = &caller.grant.holder;
        let lock = {
            let mut admission = self
                .admission
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // A lock only this map holds guards nothing.
            admission.retain(|_, lock| Arc::strong_count(lock) > 1);
            Arc::clone(admission.entry(holder.clone()).or_default())
        };
        let admitted = lock.lock_owned().await;
        let jobs = self.jobs().await?;
        let running: HashSet<&SessionId> = jobs
            .iter()
            .filter(|job| in_progress(job.state))
            .filter_map(|job| job.session.as_ref())
            .collect();
        let mut driven = self.driven.lock().unwrap_or_else(PoisonError::into_inner);
        // Only the holder's own sessions: its admission lock keeps its own
        // starts out of `running`'s blind spot, but another holder may have
        // started a turn since that `running` does not show yet. A holder's
        // ended sessions so wait for its next admission to leave, at most
        // `max_sessions` of them.
        let mine = driven.get_mut(holder).map(|mine| {
            mine.retain(|session| running.contains(session));
            &*mine
        });
        // A session whose turn already runs starts no new one: the daemon
        // refuses that prompt itself.
        let adds = session.is_none_or(|session| mine.is_none_or(|mine| !mine.contains(session)));
        let asked = u32::try_from(mine.map_or(0, HashSet::len))
            .unwrap_or(u32::MAX)
            .saturating_add(u32::from(adds));
        if adds && asked > self.limits.max_sessions {
            return Err(Refusal::Sessions {
                holder: holder.clone(),
                limit: self.limits.max_sessions,
                asked,
            }
            .into());
        }
        Ok(admitted)
    }

    /// The workspace of `session`, which must be one the daemon knows,
    /// in the caller's grant's workspaces.
    async fn reach_session(
        &self,
        caller: &Caller,
        session: &SessionId,
    ) -> Result<PathBuf, ControlError> {
        let workspace = self.workspace_of(session).await?;
        caller.grant.reach(&workspace)?;
        Ok(workspace)
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
    /// The environment a session `caller` starts or prompts runs with: the
    /// caller's, with its context one level deeper in place of any the
    /// caller's environment carries. The daemon writes the same variables
    /// from the grant the request names; they are set here too because a
    /// daemon of a build before the grant reports the same protocol version
    /// until the beta and ignores the field, and would otherwise run the
    /// session with the caller's own context.
    fn child_environment(caller: &Caller) -> ClientEnvironment {
        caller
            .environment
            .iter()
            .filter(|(name, _)| !crate::caller_context::ENV_NAMES.contains(name))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .chain(
                caller
                    .child_context()
                    .env()
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value)),
            )
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

fn prompt_content(text: &str) -> Result<Vec<ContentBlock>, ControlError> {
    if text.trim().is_empty() {
        return Err(ControlError::Invalid("the prompt is empty".into()));
    }
    Ok(vec![ContentBlock::Text {
        text: text.to_owned(),
    }])
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
