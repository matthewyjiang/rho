//! Opening a new session for `rho/session/new`, in its workspace or in a
//! worktree added for it, and registering the job of its turns.

use rho_protocol::{RpcError, SessionNewParams, SessionNewResponse};

use super::{
    grant,
    open::{config_for_new, open_session, OpenWith, Opening, Workspace},
    reopen, worktree, Admission, JobId, JobRecord, Keeping, SessionJob, SessionJobRecord, Sessions,
};
use crate::app::daemon_host::wire::session_to_wire;

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
        // Resolved before the session opens, so a malformed parent opens
        // nothing. The new session's file records the parent whether or not
        // a job of this daemon writes it, so listings fold it under that
        // session after its job is gone.
        let started_by = parent.as_ref().map(reopen::parse_session).transpose()?;
        let parent_job = started_by.as_ref().map(|parent| self.job_writing(parent));
        // A session another one starts opens with no more than its
        // parent's turn could give. A subagent run writing the parent runs
        // under its session's grant.
        let parents = parent_job
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
        let opened = open_session(
            Opening::New {
                workspace,
                agent,
                started_by,
            },
            &with,
            &self.subagents,
            &self.pull_requests,
            &self.settings,
            global,
            &job,
        )
        .await
        .map_err(|error| worktree::kept(error, worktree.as_ref()))?;
        let session = opened.runtime.session_id().clone();
        let record = JobRecord {
            // A session started from one no daemon job writes still shares
            // the budget, with no parent to show.
            admission: match parent_job {
                Some(_) => Admission::SharedBudget,
                None => Admission::Immediate,
            },
            parent: parent_job.flatten(),
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
    fn job_writing(&self, session: &rho_sdk::SessionId) -> Option<JobId> {
        match self.jobs.session_job(session) {
            SessionJob::Busy(job) => Some(job),
            SessionJob::Last(record) => Some(record.id),
            SessionJob::New => None,
        }
    }
}
