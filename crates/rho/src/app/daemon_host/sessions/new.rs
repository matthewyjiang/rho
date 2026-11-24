//! Opening a new session for `rho/session/new`, in its workspace or in a
//! worktree added for it, and registering the job of its turns.
//!
//! A session opened under a `parent` takes what a person chose for the
//! sessions its choosing session starts (see `configure.rs`): its
//! permission mode and model, before anything else can reach it, kept as
//! its own settings, and a copy of the choice itself, kept as its own, so
//! the sessions it starts take it too. The choosing session is the nearest
//! session a person or session control opened: the parent itself, or, for
//! a subagent's or workflow node's session, the session whose turn runs
//! that, found as the grant of a session it starts is, through the job
//! writing it or, once that run ended, the latest run that wrote it. A
//! parent this daemon has no job record of, such as a subagent whose job
//! was never recorded or one an in-process rho runs, is found through its
//! file instead: a subagent's session records the session that started it,
//! with that session's workspace, followed up to the first session that is
//! no subagent's. Its choice is read just before it applies: from its feed
//! while it is loaded, as it is while its turn runs, else from its file. A
//! choice that cannot be read or taken, or a recorded owner whose file
//! cannot be read, fails the `new` rather than open the session on the
//! config's, which may be looser.
//!
//! One gap remains, reached only when this daemon has no job record of the
//! parent: a parent whose file records no owner to follow, a workflow
//! node's session or a subagent of a session that had saved nothing yet,
//! is its own choosing session, and chose nothing, so the session takes
//! the config's. So does a session started without a parent, or under one
//! nothing is saved under, as any new session does.

use anyhow::Context as _;
use rho_protocol::{self as wire, ErrorCode, RpcError, SessionNewParams, SessionNewResponse};
use rho_sdk::SessionId;

use super::{
    configure::set_up_started_session,
    grant,
    open::{config_for_new, open_session, OpenWith, Opening, Workspace},
    options, reopen,
    store::store_blocking,
    worktree, Admission, JobId, JobRecord, Keeping, SessionJob, SessionJobRecord, Sessions,
};
use crate::{
    app::{daemon_host::wire::session_to_wire, session_runtime::StartedSessions},
    session::{Session as StoredSession, SessionTarget},
};

/// The session whose choice applies to the sessions a parent starts, as
/// [`Sessions::choosing_session`] finds it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Choosing {
    id: SessionId,
    /// The workspace its file is in, when the walk of saved files found it
    /// there, so its choice is read there and not from another workspace's
    /// session of the same id. `None` for one found through this daemon's
    /// jobs, or one nothing is saved under, whose file is found by id.
    saved_in: Option<std::path::PathBuf>,
}

impl Sessions {
    /// Opens a new session in `cwd`, or in a worktree added for it, as
    /// `agent`, with the client's `setup`, and registers the job of its
    /// turns.
    pub(in crate::app::daemon_host) async fn open_new(
        &self,
        params: SessionNewParams,
    ) -> Result<SessionNewResponse, RpcError> {
        let SessionNewParams {
            cwd,
            environment,
            agent,
            setup,
            parent,
            grant,
            worktree,
        } = params;
        self.refuse_when_stopping()?;
        let workspace = Workspace::parse(&cwd)?;
        // Resolved before the session opens, so a malformed parent, or
        // one whose choosing session is lost, opens nothing.
        let parent = parent.as_ref().map(reopen::parse_session).transpose()?;
        let started_by = parent.as_ref().map(|parent| self.job_writing(parent));
        let choosing = match &parent {
            Some(parent) => {
                let writer = started_by.as_ref().and_then(Option::as_ref);
                Some(self.choosing_session(parent, writer).await?)
            }
            None => None,
        };
        // A session another one starts opens with no more than its
        // parent's turn could give. A subagent run writing the parent runs
        // under its session's grant.
        let parents = started_by
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|job| self.jobs.session_grant(job));
        let opening = grant::opening(grant::asked(grant, &environment)?, parents.as_ref());
        let with = OpenWith::under(opening, &environment, setup)?;
        let global = config_for_new(&self.settings).await?;
        // Added last, once the request is otherwise sound, since nothing
        // removes it.
        let (workspace, worktree) = match worktree {
            false => (workspace, None),
            true => {
                let start = crate::worktree::Start::of(&global);
                let (workspace, added) =
                    worktree::add_for_session(&self.worktrees, &workspace, start).await?;
                (workspace, Some(added))
            }
        };
        let job = JobId::generate();
        let mut opened = open_session(
            Opening::New { workspace, agent },
            &with,
            &self.subagents,
            &self.pull_requests,
            &self.settings,
            global,
            &job,
        )
        .await
        .map_err(|error| worktree::kept(error, worktree.as_ref()))?;
        // Read now, so a change a person made while the session opened
        // counts, and applied before it is added, so no prompt finds it on
        // the config's.
        let set_up = match &choosing {
            Some(choosing) => match self.chosen_by(choosing).await {
                Ok(chosen) => {
                    set_up_started_session(
                        &mut opened.runtime,
                        &opened.config,
                        &choosing.id,
                        chosen,
                    )
                    .await
                }
                Err(error) => Err(error),
            },
            None => Ok(()),
        };
        if let Err(error) = set_up {
            // Never added, so nothing else holds the runtime.
            opened.runtime.shutdown().await;
            return Err(worktree::kept(error, worktree.as_ref()));
        }
        let session = opened.runtime.session_id().clone();
        let record = JobRecord {
            // A session started from one no daemon job writes still shares
            // the budget, with no parent to show.
            admission: match started_by {
                Some(_) => Admission::SharedBudget,
                None => Admission::Immediate,
            },
            parent: started_by.flatten(),
            keeping: match opened.runtime.persistence() {
                crate::session::SessionPersistence::Saved => Keeping::Stored,
                crate::session::SessionPersistence::Ephemeral => Keeping::MemoryOnly,
            },
            // A session that opens under a limit keeps it: no later
            // prompt widens it (#1796).
            grant_limit: (!with.grant.is_root()).then(|| with.grant.clone()),
            grant: with.grant,
            worktree: worktree.clone(),
            ..JobRecord::new_session(
                job.clone(),
                opened.runtime.workspace_root().to_path_buf(),
                session.clone(),
            )
        };
        let added = self
            .add(SessionJobRecord::Register(record), opened)
            .await
            .map_err(|error| worktree::kept(error, worktree.as_ref()))?;
        tracing::info!(%session, %job, "opened session");
        Ok(SessionNewResponse {
            session: session_to_wire(session),
            options: added.feed.options(),
            worktree,
        })
    }

    /// The job writing `session`, a new session's parent (#1714): the one
    /// under way, as the turn of a session that starts another is, else the
    /// latest job of its turns. `None` for a session no job of this daemon
    /// writes, such as one an in-process rho runs.
    fn job_writing(&self, session: &SessionId) -> Option<JobId> {
        match self.jobs.session_job(session) {
            SessionJob::Busy(job) => Some(job),
            SessionJob::Last(record) => Some(record.id),
            SessionJob::New => None,
        }
    }

    /// The session whose choice applies to a session `parent` starts, when
    /// `writer` is the job writing `parent`: the nearest session job's at
    /// or above `writer`, as a session `parent` starts takes that one's
    /// grant, so a subagent or workflow node of a session starts sessions
    /// under that session's choice. When no job of this daemon writes
    /// `parent`, the same of the latest run that wrote it and has ended
    /// (`JobRegistry::last_run_writing`); when no job of this daemon ever
    /// wrote it, the session that owns it as its file records it (see
    /// [`owning_session`]). A run whose session job is gone is
    /// `SessionOpenFailed`: the choice that applies cannot be found.
    async fn choosing_session(
        &self,
        parent: &SessionId,
        writer: Option<&JobId>,
    ) -> Result<Choosing, RpcError> {
        let Some(writer) = writer
            .cloned()
            .or_else(|| self.jobs.last_run_writing(parent))
        else {
            let parent = parent.clone();
            return store_blocking(FIND_OWNER, move || owning_session(parent)).await;
        };
        let id = self.jobs.nearest_session(&writer).ok_or_else(|| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!(
                    "could not find the session whose turn runs job {writer}, which writes or wrote {parent}, so not what it chose for the sessions it starts"
                ),
            )
        })?;
        Ok(Choosing { id, saved_in: None })
    }

    /// What `choosing` chose for the sessions it starts, as it is now: from
    /// its feed while it is loaded, as it is while its turn or that of one
    /// of its subagents runs, else from its file, in the workspace the walk
    /// found it in, if it did. A session nothing is saved under chose
    /// nothing. A choice its file keeps but this rho cannot read is a mode
    /// this rho does not know, which the new session refuses to open in: it
    /// may be stricter than the config.
    async fn chosen_by(
        &self,
        choosing: &Choosing,
    ) -> Result<wire::StartedSessionDefaults, RpcError> {
        let loaded = self.loaded().get(&choosing.id).cloned();
        if let Some(session) = loaded {
            return Ok(session.feed.started_session_defaults());
        }
        let Choosing { id, saved_in } = choosing.clone();
        let saved = store_blocking(READ_CHOICE, move || {
            let cwd = match saved_in {
                Some(cwd) => cwd,
                None => match StoredSession::workspace_of(id.as_str())? {
                    Some(cwd) => cwd,
                    None => return Ok(None),
                },
            };
            StartedSessions::saved_by_id(&cwd, id.as_str()).map(Some)
        })
        .await?;
        Ok(saved
            .as_ref()
            .map(options::started_session_defaults)
            .unwrap_or_default())
    }
}

/// What a failed read of a starting session's choice could not do.
const READ_CHOICE: &str =
    "read what the session that starts this one chose for the sessions it starts";

/// What a failed walk of [`owning_session`] could not do.
const FIND_OWNER: &str = "find the session that owns the subagent that starts this one";

/// The session whose choice applies to a session `parent` starts when no
/// job of this daemon ever wrote `parent`: `parent` itself, unless its file
/// records it as a subagent's session, whose parent the file names, with
/// its workspace (the one a delete cascades over), followed up to the first
/// session that is no subagent's. That one a person or session control
/// opened. A parent nothing is saved under, or whose file records no owner
/// (a workflow node's session, a subagent of a session that had saved
/// nothing yet), is its own. An owner a file names but that cannot be read
/// fails, so the `new` fails rather than open the session on the config's.
/// Blocking.
fn owning_session(parent: SessionId) -> anyhow::Result<Choosing> {
    let Some(cwd) = StoredSession::workspace_of(parent.as_str())? else {
        return Ok(Choosing {
            id: parent,
            saved_in: None,
        });
    };
    let mut session = SessionTarget::new(parent.as_str(), cwd);
    let mut seen = std::collections::HashSet::new();
    loop {
        let Some(owner) = StoredSession::subagent_parent_by_id(&session.cwd, &session.id)
            .with_context(|| format!("could not read which session owns {}", session.id))?
        else {
            return Ok(Choosing {
                id: SessionId::from_string(session.id)?,
                saved_in: Some(session.cwd),
            });
        };
        anyhow::ensure!(
            seen.insert(session.id.clone()),
            "the subagent sessions above {} name each other as their parent",
            session.id
        );
        session = owner;
    }
}

#[cfg(test)]
#[path = "new_tests.rs"]
mod tests;
