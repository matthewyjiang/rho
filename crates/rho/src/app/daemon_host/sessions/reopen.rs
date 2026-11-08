//! Reopening saved sessions on demand (#1661). A prompt or an attach for a
//! saved session the daemon has not loaded opens it from its file, in its
//! own workspace, and records its turns on the session's last session job,
//! or a new one when no daemon ran it. Cancel, steer, answer, and hold never
//! reopen one: a session that is not loaded has no turn to act on.
//!
//! A session another process has open is refused as in use, and a
//! subagent's or workflow node's session as its parent's to continue.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_protocol::{ErrorCode, RpcError};
use rho_sdk::SessionId;
use tokio::sync::OwnedMutexGuard;

use super::{
    open::{open_session, Opening},
    open_failed, writing, LoadedSession, SessionJobRecord, Sessions, TurnSlot,
};
use crate::{
    app::daemon_host::{
        jobs::{JobId, JobRecord, SessionJob},
        wire::session_from_wire,
    },
    session::{Session as StoredSession, SessionInUse, SessionOrigin},
    session_environment::Ambient,
};

impl Sessions {
    /// The session `id` names, loaded: one the daemon has loaded, or a saved
    /// one it reopens with its children starting in `environment`. Two
    /// requests for the same unloaded session open it once; different
    /// sessions open at the same time.
    pub(super) async fn session(
        &self,
        id: &rho_protocol::SessionId,
        environment: &Ambient,
    ) -> Result<Arc<LoadedSession>, RpcError> {
        let id = parse_session(id)?;
        if let Some(session) = self.loaded().get(&id) {
            return Ok(Arc::clone(session));
        }
        let _reopening = self.reopening.lock(&id).await;
        // A request that held the lock first may have opened it.
        let loaded = self.loaded().get(&id).cloned();
        match loaded {
            Some(session) => Ok(session),
            None => self.reopen(&id, environment).await,
        }
    }

    /// Runs `use_session` on the session `id` names, loaded as
    /// [`Self::session`] loads it, under its turn lock and marked in use, so
    /// the idle unload cannot take it meanwhile (#1661). A session the
    /// unload took after the lookup is looked up again, which reopens it:
    /// an unloaded session is never in `loaded`.
    pub(super) async fn enter_session<T>(
        &self,
        id: &rho_protocol::SessionId,
        environment: &Ambient,
        use_session: impl FnOnce(&LoadedSession, &mut TurnSlot) -> Result<T, RpcError>,
    ) -> Result<(Arc<LoadedSession>, T), RpcError> {
        loop {
            let session = self.session(id, environment).await?;
            let Some(mut turn) = session.enter() else {
                continue;
            };
            let used = use_session(&session, &mut turn);
            drop(turn);
            return used.map(|used| (session, used));
        }
    }

    /// The loaded session `id` names, or `None` for a saved session this
    /// daemon has not loaded. An id nothing is saved under is
    /// `SessionNotFound`.
    pub(super) async fn loaded_session(
        &self,
        id: &rho_protocol::SessionId,
    ) -> Result<Option<Arc<LoadedSession>>, RpcError> {
        let id = parse_session(id)?;
        if let Some(session) = self.loaded().get(&id) {
            return Ok(Some(Arc::clone(session)));
        }
        saved_workspace(&id).await.map(|_| None)
    }

    /// Reopens saved session `id` from its file, with its children
    /// starting in `environment`, and records its turns on the session's
    /// last job, or a new one when no daemon has run it.
    async fn reopen(
        &self,
        id: &SessionId,
        environment: &Ambient,
    ) -> Result<Arc<LoadedSession>, RpcError> {
        self.refuse_when_stopping()?;
        let last = match self.jobs.session_job(id) {
            SessionJob::Busy(job) => return Err(writing(&job, id)),
            SessionJob::Last(record) => Some(record),
            SessionJob::New => None,
        };
        let stored = saved_session(id).await?;
        let record = match last {
            Some(record) => SessionJobRecord::Reuse(record),
            None => SessionJobRecord::Register(JobRecord::new_session(
                JobId::generate(),
                stored.cwd().to_path_buf(),
                id.clone(),
            )),
        };
        let job = record.job().id.clone();
        let opened = open_session(
            Opening::Saved(stored),
            environment.clone(),
            &self.subagents,
            &job,
        )
        .await
        .map_err(open_failed)?;
        let session = self.add(record, opened).await?;
        tracing::info!(session = %id, %job, "reopened session");
        Ok(session)
    }
}

/// One lock per saved session being reopened, so concurrent requests for a
/// session reopen it once while different sessions reopen at the same time.
/// The idle unload holds a session's through its teardown, so a reopen does
/// not find the file still held by the old runtime. A session's lock is
/// removed once nothing holds or waits on it.
#[derive(Default)]
pub(super) struct ReopenLocks(Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>);

impl ReopenLocks {
    /// Waits for `id`'s lock, held until the guard drops.
    pub(super) async fn lock(&self, id: &SessionId) -> ReopenGuard<'_> {
        let lock = Arc::clone(self.map().entry(id.clone()).or_default());
        ReopenGuard {
            locks: self,
            id: id.clone(),
            held: lock.lock_owned().await,
        }
    }

    fn map(&self) -> MutexGuard<'_, HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>> {
        // Only inserted into and removed from whole.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.map().is_empty()
    }
}

/// A held [`ReopenLocks`] lock.
pub(super) struct ReopenGuard<'a> {
    locks: &'a ReopenLocks,
    id: SessionId,
    held: OwnedMutexGuard<()>,
}

impl Drop for ReopenGuard<'_> {
    fn drop(&mut self) {
        let mut map = self.locks.map();
        // The map's and this guard's are the only clones unless a request
        // waits on the lock, and clones are taken only under the map's lock,
        // held here. Removing a lock someone waits on would let the next
        // request reopen beside that waiter. Requests never give up: the
        // transport runs every request to its end. The sweeper aborted at
        // the daemon's exit can leave one entry, which nothing reads then.
        if Arc::strong_count(OwnedMutexGuard::mutex(&self.held)) == 2 {
            map.remove(&self.id);
        }
    }
}

/// The workspace of the session saved under `id`; `SessionNotFound` when
/// nothing is.
async fn saved_workspace(id: &SessionId) -> Result<PathBuf, RpcError> {
    let lookup = {
        let id = id.clone();
        tokio::task::spawn_blocking(move || StoredSession::workspace_of(id.as_str())).await
    };
    match lookup {
        Ok(Ok(Some(cwd))) => Ok(cwd),
        Ok(Ok(None)) => Err(not_found(id)),
        Ok(Err(error)) => Err(lookup_failed(&error)),
        Err(error) => Err(lookup_failed(&error.into())),
    }
}

/// The session saved under `id`, opened in its own workspace.
async fn saved_session(id: &SessionId) -> Result<StoredSession, RpcError> {
    let cwd = saved_workspace(id).await?;
    let id = id.clone();
    tokio::task::spawn_blocking(move || open_saved(&cwd, &id))
        .await
        .map_err(|error| lookup_failed(&error.into()))?
}

/// Opens the session saved under `id` in `cwd`'s workspace, unless another
/// process has it open or it is a child session, which only its parent
/// continues: a client's turns there would interleave with the parent's
/// when it resumes the child. Blocking.
fn open_saved(cwd: &Path, id: &SessionId) -> Result<StoredSession, RpcError> {
    let stored = match StoredSession::open_exact_in_workspace(cwd, id.as_str()) {
        Ok(Some((stored, _histories))) => stored,
        // Deleted since the index named it.
        Ok(None) => return Err(not_found(id)),
        Err(error) if error.is::<SessionInUse>() => {
            return Err(RpcError::new(ErrorCode::SessionInUse, format!("{error:#}")))
        }
        Err(error) => return Err(open_failed(error)),
    };
    let child = match stored.origin().map_err(open_failed)? {
        Some(SessionOrigin::Subagent) => Some("a subagent run"),
        Some(SessionOrigin::WorkflowNode) => Some("a workflow node"),
        Some(
            SessionOrigin::Interactive
            | SessionOrigin::Acp
            | SessionOrigin::Daemon
            | SessionOrigin::Run
            | SessionOrigin::Unknown,
        )
        | None => None,
    };
    match child {
        Some(child) => Err(RpcError::new(
            ErrorCode::SessionOpenFailed,
            format!("session {id} is {child}'s; only the session that started it continues it"),
        )),
        None => Ok(stored),
    }
}

fn parse_session(id: &rho_protocol::SessionId) -> Result<SessionId, RpcError> {
    session_from_wire(id).map_err(|_| RpcError::new(ErrorCode::InvalidParams, "not a session id"))
}

fn not_found(id: &SessionId) -> RpcError {
    RpcError::new(
        ErrorCode::SessionNotFound,
        format!("no session is saved under {id}"),
    )
}

fn lookup_failed(error: &anyhow::Error) -> RpcError {
    RpcError::new(
        ErrorCode::InternalError,
        format!("could not look the session up: {error:#}"),
    )
}

#[cfg(test)]
#[path = "reopen_tests.rs"]
mod tests;
