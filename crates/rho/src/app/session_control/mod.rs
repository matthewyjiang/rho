//! The session-control core: what an agent may do to the daemon's sessions,
//! under one policy, whichever front end asks. `rho mcp serve`
//! (`app::mcp_server`) and the native `sessions` tool's control actions
//! (`tools::sessions::control`) are thin adapters over it (#1714).
//!
//! [`SessionControl`] serves eight actions ([`Action`]): `list`, `read`,
//! `new`, `prompt`, `steer`, `cancel`, `wait`, and `answer`. Each call
//! carries its [`Caller`]: the transport's [`Grant`], narrowed by the scope of the
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
//! as subagent runs do (#1714). `new` may ask the daemon to open the session
//! in a git worktree of its own (#1673); a session in a worktree counts as
//! in the workspace the worktree was added from as well as its own, for the
//! grant and `list`'s filter.
//!
//! No action answers a session's approvals or plan approvals: a turn that
//! asks waits for a person, and `wait` reports it waiting for input. The
//! one input an action answers is a question, and only one asked by a
//! session the caller's session started: `answer`, which `wait`'s `inputs`
//! name the question for (see [`answer`]). The daemon holds the core to
//! that, and decides who may answer. Its connections declare an agent's
//! role ([`ConnectAs::Agent`]), which the daemon refuses
//! `rho/session/answer` and `rho/session/hold` from (#1780), and
//! `rho/session/answer_question`, the one answering method open to an
//! agent, takes only a question's answers, from the session whose job
//! started the asker's, and never for a question only the user may answer,
//! as an MCP server's elicitation is. `rho mcp serve` offers no `answer`.
//!
//! The daemon is reached as [`Reach`] allows: `rho mcp serve` starts it
//! when none runs, as any client does, and the native tool reaches only one
//! already running. Requests share one connection; `read` and `wait` observe on a
//! connection of their own, so calls on the same session never replace each
//! other's attachment. Observing never loads a session, so reading one
//! never starts its MCP servers at the reader's nesting depth.
//!
//! Front ends that follow a session beyond one `wait`, as `rho mcp serve`'s
//! webhook watches do, check the caller as `wait` does
//! ([`SessionControl::check_follow`]) and keep a [`Follow`], which reports
//! each new reason the session is done and why the daemon ended it. Their
//! checks on nobody's call, as before a webhook POST, reach only a daemon
//! already running ([`DaemonUse::RunningOnly`]), so they never start one a
//! person stopped.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use rho_protocol::{
    methods::{JobList, SessionCancel, SessionConfigure, SessionNew, SessionPrompt, SessionSteer},
    ClientEnvironment, ClientError, ContentBlock, JobId, JobKind, JobListParams, JobSummary,
    ModelRef, SessionCancelParams, SessionChange, SessionConfigureParams, SessionId,
    SessionNewParams, SessionPromptParams, SessionSetup, SessionSteerParams,
};
use serde_json::Value;

use super::{
    daemon_cli::Unreachable,
    daemon_client::{Daemon, DaemonError},
};
use crate::{app::session_options::OfferedModels, daemon::ConnectAs};

mod answer;
mod catalog;
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
pub(crate) use dispatch::{parse_arguments, schema_of};
pub(crate) use follow::{Connect, Follow, FollowEnd, Followed, NewsKey};
use params::{
    AnswerParams, AnswerResult, CancelParams, CancelResult, ListParams, ListResult, NewParams,
    NewResult, PromptParams, PromptResult, ReadParams, ReadResult, SessionEntry, SteerParams,
    SteerResult, WaitParams,
};
pub(crate) use params::{WaitResult, MAX_WAIT_SECS};
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

impl ControlError {
    /// Whether the session is one the daemon does not know: the core found
    /// no job or saved session for it, or the daemon answered that no
    /// saved session has its id, as it does once the session was deleted.
    pub(crate) fn is_unknown_session(&self) -> bool {
        match self {
            Self::UnknownSession(_) => true,
            Self::Daemon(DaemonError::Request(ClientError::Rpc { error, .. })) => {
                error.code == rho_protocol::ErrorCode::SessionNotFound.code()
            }
            Self::Refused(_) | Self::Invalid(_) | Self::Daemon(_) => false,
        }
    }
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

/// Which daemon a check outside the actions asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DaemonUse {
    /// The daemon as this core reaches it, which may start one: for a
    /// caller's call.
    Call,
    /// Only a daemon already running: for a check nobody's call asked for,
    /// as before a webhook POST, which must not start one a person stopped.
    RunningOnly,
}

/// The core. One per process; every caller shares it.
pub(crate) struct SessionControl {
    daemon: Daemon,
    /// How `daemon`, and the connections `read` and `wait` open, reach it.
    reach: Reach,
    /// The daemon only while it runs, for checks no call asked for
    /// ([`DaemonUse::RunningOnly`]) by a core that may start one. Made on
    /// first use; a core that reaches only a running daemon uses `daemon`.
    running: OnceLock<Daemon>,
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
            running: OnceLock::new(),
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
            CheckedCall::Wait(params) => json(self.wait(caller, params).await?),
            CheckedCall::Answer {
                session,
                request,
                answer,
                by,
            } => json(self.answer(session, request, answer, by).await?),
        }
    }

    /// The daemon's sessions the grant reaches, the most recently changed
    /// first, whoever opened them. Those it returns that the daemon does
    /// not have loaded take their title from its catalog; a catalog the
    /// daemon cannot read costs the titles, not the list.
    async fn list(&self, caller: &Caller, params: ListParams) -> Result<ListResult, ControlError> {
        let limit = params.limit()?;
        let workspace = params.workspace.as_deref().map(canonical).transpose()?;
        let jobs = self.jobs().await?;
        let writers: HashMap<JobId, SessionId> = jobs
            .iter()
            .filter_map(|job| Some((job.id.clone(), job.session.clone()?)))
            .collect();
        let mut jobs: Vec<JobSummary> = jobs
            .into_iter()
            .filter(|job| job.kind == JobKind::Session && job.session.is_some())
            .filter(|job| {
                workspaces_of(job)
                    .into_iter()
                    .any(|place| caller.grant.scope.workspaces.allows(place))
            })
            .filter(|job| {
                workspace
                    .as_ref()
                    .is_none_or(|only| workspaces_of(job).into_iter().any(|place| place == only))
            })
            .filter(|job| params.keeps(job))
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.updated_at));
        let more = jobs.len() > limit;
        jobs.truncate(limit);
        // A loaded session's title is the saved one already, so only the
        // unloaded ones are looked up.
        let unloaded = jobs
            .iter()
            .filter(|job| job.live.is_none())
            .filter_map(|job| Some((job.session.as_ref()?, job.workspace.as_path())));
        let mut saved = match catalog::titles(&self.daemon, unloaded).await {
            Ok(saved) => saved,
            Err(error) => {
                tracing::warn!(%error, "could not read the saved sessions' titles; listing without them");
                HashMap::new()
            }
        };
        let sessions = jobs
            .into_iter()
            .filter_map(|job| SessionEntry::of_job(job, &writers, &mut saved))
            .collect();
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
                worktree: params.worktree,
            })
            .await?;
        // Before the first turn, so nothing runs in a checkout the caller
        // asked to keep out of.
        crate::worktree::require(params.worktree, opened.worktree.as_ref())
            .map_err(|error| ControlError::Invalid(error.to_string()))?;
        if let Some(model) = params.model {
            let model = choose_model(&opened.options.model, &model)?;
            self.daemon
                .request::<SessionConfigure>(&SessionConfigureParams {
                    session: opened.session.clone(),
                    change: SessionChange::Model {
                        model,
                        auth: None,
                        setup: true,
                    },
                })
                .await?;
        }
        self.start_turn(caller, &opened.session, content).await?;
        Ok(NewResult {
            session: opened.session,
            workspace,
            worktree: opened.worktree,
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

    /// Waits up to the call's timeout for the session to settle or ask for
    /// input, then reports where it is, with a cursor the next `wait`
    /// resumes from.
    async fn wait(&self, caller: &Caller, params: WaitParams) -> Result<WaitResult, ControlError> {
        let timeout = params.timeout()?;
        let after = params.cursor()?;
        let reach = match caller.daemon {
            DaemonUse::Call => self.reach,
            DaemonUse::RunningOnly => Reach::Running,
        };
        follow::wait(reach, params.session, after, timeout).await
    }

    /// Checks that `caller` may follow `session` as `wait` does: read
    /// access, a session the daemon knows, in a workspace the grant reaches,
    /// asking the daemon as `used` says.
    pub(crate) async fn check_follow(
        &self,
        caller: &Caller,
        session: &SessionId,
        used: DaemonUse,
    ) -> Result<(), ControlError> {
        caller.grant.permit(Action::Wait)?;
        reach_session_on(self.daemon_for(used), caller, session)
            .await
            .map(drop)
    }

    /// The daemon a check `used` so asks.
    fn daemon_for(&self, used: DaemonUse) -> &Daemon {
        match (used, self.reach) {
            (DaemonUse::Call, Reach::StartIfNeeded | Reach::Running)
            | (DaemonUse::RunningOnly, Reach::Running) => &self.daemon,
            (DaemonUse::RunningOnly, Reach::StartIfNeeded) => self
                .running
                .get_or_init(|| Daemon::reaching(Reach::Running, ConnectAs::Agent)),
        }
    }

    /// Where `session` is now, as a `wait` with no timeout reports it.
    /// Check the caller with [`Self::check_follow`] first.
    pub(crate) async fn now(&self, session: &SessionId) -> Result<WaitResult, ControlError> {
        follow::wait(self.reach, session.clone(), None, std::time::Duration::ZERO).await
    }

    /// Follows `session` from its start, reaching the daemon as `connect`
    /// says; [`Connect::Reach`] with this core's own reach may start one.
    /// Check the caller with [`Self::check_follow`] first.
    pub(crate) async fn follow(
        &self,
        session: &SessionId,
        connect: Connect,
    ) -> Result<Follow, ControlError> {
        Follow::open(connect, session.clone(), None).await
    }

    /// How this core reaches the daemon.
    pub(crate) fn reach(&self) -> Reach {
        self.reach
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
                author: rho_protocol::PromptAuthor::Person,
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
            .filter(|job| job.state.in_progress())
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
    /// in the caller's grant's workspaces. A session in a worktree is in
    /// them when its project is.
    async fn reach_session(
        &self,
        caller: &Caller,
        session: &SessionId,
    ) -> Result<PathBuf, ControlError> {
        reach_session_on(self.daemon_for(caller.daemon), caller, session).await
    }

    async fn jobs(&self) -> Result<Vec<JobSummary>, ControlError> {
        jobs_of(&self.daemon).await
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

/// [`SessionControl::reach_session`], asking `daemon`.
async fn reach_session_on(
    daemon: &Daemon,
    caller: &Caller,
    session: &SessionId,
) -> Result<PathBuf, ControlError> {
    let place = place_of(daemon, session).await?;
    if !caller.grant.scope.workspaces.allows(&place.project) {
        caller.grant.reach(&place.workspace)?;
    }
    Ok(place.workspace)
}

/// Where `session` works: its job's workspace and project, or, for a
/// saved session no daemon has run, the catalog's workspace.
async fn place_of(daemon: &Daemon, session: &SessionId) -> Result<Place, ControlError> {
    let jobs = jobs_of(daemon).await?;
    if let Some(job) = jobs
        .iter()
        .find(|job| job.session.as_ref() == Some(session))
    {
        return Ok(Place {
            workspace: job.workspace.clone(),
            project: job.project().to_path_buf(),
        });
    }
    catalog::scan(daemon, /*cwd*/ None, |saved| {
        (&saved.session == session).then(|| Place {
            workspace: saved.cwd.clone(),
            project: saved.cwd,
        })
    })
    .await?
    .ok_or_else(|| ControlError::UnknownSession(session.clone()))
}

async fn jobs_of(daemon: &Daemon) -> Result<Vec<JobSummary>, ControlError> {
    Ok(daemon.request::<JobList>(&JobListParams {}).await?.jobs)
}

/// Where a session works: its workspace, and the project it works for,
/// which for a session in a worktree is the workspace the worktree was
/// added from (`JobSummary::project`).
struct Place {
    workspace: PathBuf,
    project: PathBuf,
}

/// The workspaces `job` counts as in, for the grant and `list`'s filter: its
/// own and its project.
fn workspaces_of(job: &JobSummary) -> [&Path; 2] {
    [job.workspace.as_path(), job.project()]
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
    options: &rho_protocol::ModelOptions,
    wanted: &str,
) -> Result<ModelRef, ControlError> {
    let offered = OfferedModels::new(options);
    if let Some(choice) = offered.by_reference(wanted) {
        return Ok(choice.model.clone());
    }
    let by_id: Vec<_> = options
        .available
        .iter()
        .filter(|choice| offered.answers(choice, wanted))
        .collect();
    match by_id.as_slice() {
        [choice] => Ok(choice.model.clone()),
        _ => {
            let offered: Vec<String> = options
                .available
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
