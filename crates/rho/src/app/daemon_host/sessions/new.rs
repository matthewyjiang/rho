//! Opening a new session for `rho/session/new`, in its workspace or in a
//! worktree added for it, and registering the job of its turns.
//!
//! A session opened under a `parent` takes what a person chose for the
//! sessions its choosing session starts (see `configure.rs`): its
//! permission mode and model, before anything else can reach it, kept as
//! its own settings, and the choice itself, kept as its own, so the
//! sessions it starts take it too. The choosing session is the nearest
//! session a person or session control opened: the parent itself, or, for
//! a subagent's or workflow node's session, the session whose turn runs
//! that, found as the grant of a session it starts is. Its choice is read
//! just before it applies: from its feed while it is loaded, as it is while
//! its turn runs, else from its file. A choice that cannot be read or taken
//! fails the `new` rather than open the session on the config's, which may
//! be looser. Without a parent, or with one no job of this daemon writes
//! and nothing is saved under, the session takes the config's, as any new
//! session does.

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
    app::{daemon_host::wire::session_to_wire, session_runtime::StartedSessionDefaults},
    session::Session as StoredSession,
};

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
        let choosing = parent
            .as_ref()
            .map(|parent| {
                let writer = started_by.as_ref().and_then(Option::as_ref);
                self.choosing_session(parent, writer)
            })
            .transpose()?;
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
                    set_up_started_session(&mut opened.runtime, &opened.config, choosing, chosen)
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
    /// under that session's choice. `parent` itself when no job of this
    /// daemon writes it. A writer whose session job is gone is
    /// `SessionOpenFailed`: the choice that applies cannot be found.
    fn choosing_session(
        &self,
        parent: &SessionId,
        writer: Option<&JobId>,
    ) -> Result<SessionId, RpcError> {
        let Some(writer) = writer else {
            return Ok(parent.clone());
        };
        self.jobs.nearest_session(writer).ok_or_else(|| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!(
                    "could not find the session whose turn runs job {writer}, which writes {parent}, so not what it chose for the sessions it starts"
                ),
            )
        })
    }

    /// What `choosing` chose for the sessions it starts, as it is now: from
    /// its feed while it is loaded, as it is while its turn or that of one
    /// of its subagents runs, else from its file. A session nothing is
    /// saved under chose nothing. A choice its file keeps but that does not
    /// read fails, naming why: it may be stricter than the config.
    async fn chosen_by(
        &self,
        choosing: &SessionId,
    ) -> Result<wire::StartedSessionDefaults, RpcError> {
        let loaded = self.loaded().get(choosing).cloned();
        if let Some(session) = loaded {
            return Ok(session.feed.started_session_defaults());
        }
        let id = choosing.clone();
        let saved = store_blocking(READ_CHOICE, move || {
            let Some(cwd) = StoredSession::workspace_of(id.as_str())? else {
                return Ok(None);
            };
            StartedSessionDefaults::saved_by_id(&cwd, id.as_str()).map(Some)
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

#[cfg(test)]
#[path = "new_tests.rs"]
mod tests;
