//! Observing sessions (`rho/session/observe`): following a session's events
//! as an attach does, for a client that only reads it, such as the `read`
//! and `wait` tools of `rho mcp serve` (#1672). Observing never loads a
//! session. One the daemon has loaded is followed as an attach follows it,
//! keeping the environment it runs with. One a subagent or workflow agent
//! node run is writing is followed through the run's stream, from when the
//! run opens it until the run ends (#1668; see `child_feeds`). A saved one it has not loaded is
//! opened only to be read, starting none of its MCP servers, OpenAPI
//! sources, or hooks, replayed as its file keeps it with how its last turn
//! ended, and shut down again; the client is then detached as
//! [`DetachReason::NotLoaded`]. A reopen would start the session's servers
//! with the reader's environment, at the reader's nesting depth rather
//! than the session's own.

use rho_protocol::{
    Cursor, DetachReason, RpcError, SessionAttachResponse, SessionObserveParams, Subscription,
};
use rho_sdk::SessionId;

use super::{
    attach_response,
    open::open_to_read,
    opening_feed,
    options::available_auths,
    reopen::{parse_session, saved_session, SavedFor},
    worktree::{Restore, Restored},
    writing, Sessions,
};
use crate::app::daemon_host::jobs::{JobId, JobRecord, SessionJob};

impl Sessions {
    /// Subscribes a client to a session's events without loading it: a
    /// loaded session's as [`Self::attach`] does, else a replay of what the
    /// session saved, after which the client is detached.
    pub(in crate::app::daemon_host) async fn observe(
        &self,
        params: SessionObserveParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        self.refuse_when_stopping()?;
        let id = parse_session(&params.session)?;
        let after = params.after.as_ref();
        loop {
            let loaded = self.loaded().get(&id).cloned();
            if let Some(session) = loaded {
                // Under the turn lock and marked in use, as an attach, so
                // the idle unload cannot take it meanwhile. Taken already:
                // looked up again, which no longer finds it loaded.
                let Some(_turn) = session.enter() else {
                    continue;
                };
                return Ok(attach_response(
                    &session.workspace,
                    session.feed.attach(after),
                ));
            }
            // A subagent run under way writes it: its stream is the run's.
            if let Some(observed) = self.subagents.feeds().attach(&id, after) {
                return Ok(observed);
            }
            // Held while reading, so no request loads the session meanwhile
            // and the read does not find its file held.
            let _reopening = self.reopening.lock(&id).await;
            // A request that held the lock first may have loaded it.
            if self.loaded().contains_key(&id) {
                continue;
            }
            return self.read_unloaded(&id, after).await;
        }
    }

    /// The replay of saved session `id`, which the daemon has not loaded,
    /// as a client resuming from `after` gets it, then nothing: the
    /// session is read and shut down again (see [`open_to_read`]). Its
    /// reopen lock is held.
    async fn read_unloaded(
        &self,
        id: &SessionId,
        after: Option<&Cursor>,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        let mut last = match self.jobs.session_job(id) {
            SessionJob::Busy(job) => return Err(writing(&job, id)),
            SessionJob::Last(record) => Some(*record),
            SessionJob::New => None,
        };
        let restored = match &mut last {
            Some(record) => self.restore_worktree(record, Restore::Reading).await?,
            None => Restored::Nothing,
        };
        let observed = self.read_saved(id, after, last.clone()).await;
        // Removed again however the read went.
        if let (Restored::ForReading, Some(record)) = (restored, &last) {
            Self::end_read_checkout(record).await;
        }
        observed
    }

    /// The replay of saved session `id` whose last job is `last`, as
    /// [`Self::read_unloaded`] answers it once its worktree is on disk.
    async fn read_saved(
        &self,
        id: &SessionId,
        after: Option<&Cursor>,
        last: Option<JobRecord>,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        let stored = saved_session(id, SavedFor::Reading).await?;
        // A session no daemon has run is idle, with no last turn. Its job
        // is not registered: reading it records nothing.
        let job = last.unwrap_or_else(|| {
            JobRecord::new_session(JobId::generate(), stored.cwd().to_path_buf(), id.clone())
        });
        let opened = open_to_read(stored, &self.subagents, &self.settings, &job.id).await?;
        let (feed, _) = opening_feed(
            &job,
            &opened.runtime,
            &opened.config,
            &available_auths(opened.runtime.credentials()).await,
        );
        feed.close(DetachReason::NotLoaded);
        let observed = attach_response(opened.runtime.workspace_root(), feed.attach(after));
        opened.runtime.shutdown().await;
        Ok(observed)
    }
}
