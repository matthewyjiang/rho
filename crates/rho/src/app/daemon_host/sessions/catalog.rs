//! The session catalog over the socket (#1670, #1667): `rho/session/list`,
//! `rho/session/rename`, and `rho/session/delete`, over the session store's
//! catalog, titles, and delete, which ACP's `session/list` and
//! `session/delete` and `rho sessions` use too.
//!
//! A rename tells a loaded session's attached clients the new title. A
//! delete must unload a loaded session first, since its runtime holds the
//! file's lease: it refuses one with a turn or background work under way,
//! or a settings change queued or applying, or whose delete the store would
//! refuse, holds it from prompts and changes while it tears it down, and
//! then detaches its clients as `deleted`. The session's loaded forks, whose
//! files the store deletes with it, are checked and unloaded with it: one
//! that refuses refuses the delete, and leaves them all loaded.

use std::{num::NonZeroU32, num::NonZeroUsize, sync::Arc};

use rho_protocol::{
    self as wire, DetachReason, ErrorCode, RpcError, SessionDeleteParams, SessionDeleteResponse,
    SessionListParams, SessionListResponse, SessionRenameParams, SessionRenameResponse,
};
use rho_sdk::SessionId;
use tokio::sync::OwnedMutexGuard;

use super::{
    being_deleted,
    reopen::{not_found, parse_session},
    stopping,
    store::{store_blocking, store_failed},
    t3_wake::WakeSignal,
    writing, Held, Hold, LoadedSession, Sessions,
};
use crate::{
    app::{daemon_host::jobs::SessionJob, session_runtime::SessionRuntime},
    session::{
        catalog::{self, CatalogError},
        ExactDelete, NoSessionFound, Session as StoredSession, SessionSummary,
    },
};

/// The most sessions one `rho/session/list` page holds.
//
// Measured on a dev machine's store: its 249 listable sessions encode to
// 86 KB of summaries, 393 B at most each, so a full page is about 400 KB,
// far under the 16 MiB line limit (`rho_protocol::MAX_LINE_BYTES`), and four
// times that whole store still lists in one page. A client that wants more
// follows `next_cursor`.
const MAX_PAGE_SIZE: usize = 1000;

impl Sessions {
    /// One page of the saved sessions a person can resume, newest first.
    pub(in crate::app::daemon_host) async fn list(
        &self,
        params: SessionListParams,
    ) -> Result<SessionListResponse, RpcError> {
        let SessionListParams { cwd, cursor, limit } = params;
        let limit = page_size(limit)?;
        let page = store_blocking("list the sessions", move || {
            Ok(catalog::list(cwd.as_deref(), cursor.as_deref(), limit))
        })
        .await?
        .map_err(|error| match error {
            CatalogError::RelativeCwd | CatalogError::UnknownCursor(_) => {
                invalid(error.to_string())
            }
            CatalogError::Store(error) => store_failed("list the sessions", &error),
        })?;
        Ok(SessionListResponse {
            sessions: page.sessions.into_iter().map(summary_to_wire).collect(),
            next_cursor: page.next_cursor,
        })
    }

    /// Saves a session's title, then tells the clients attached to it, if
    /// the daemon has it loaded.
    pub(in crate::app::daemon_host) async fn rename(
        &self,
        params: SessionRenameParams,
    ) -> Result<SessionRenameResponse, RpcError> {
        let id = parse_session(&params.session)?;
        let renamed = {
            let id = id.clone();
            store_blocking("rename the session", move || {
                let Some(cwd) = StoredSession::workspace_of(id.as_str())? else {
                    return Ok(None);
                };
                match StoredSession::set_title(&cwd, id.as_str(), &params.title) {
                    Ok(update) => Ok(Some(update.title)),
                    // Deleted since the lookup.
                    Err(error) if error.is::<NoSessionFound>() => Ok(None),
                    Err(error) => Err(error),
                }
            })
            .await?
        };
        let title = renamed.ok_or_else(|| not_found(&id))?;
        let loaded = self.loaded().get(&id).cloned();
        if let Some(session) = loaded {
            session
                .feed
                .publish([wire::SessionEvent::TitleChanged { title }]);
        }
        Ok(SessionRenameResponse {})
    }

    /// Deletes a saved session and the sessions it started, unloading it
    /// and its loaded forks first.
    pub(in crate::app::daemon_host) async fn delete(
        &self,
        params: SessionDeleteParams,
    ) -> Result<SessionDeleteResponse, RpcError> {
        self.refuse_when_stopping()?;
        let id = parse_session(&params.session)?;
        // Both held until the files are gone: the stop waits for the first,
        // and a request that would reopen the session, or link or unlink a
        // pull request for it, waits for the second, then finds nothing
        // saved. A new fork holds the session's reopen lock until it is
        // loaded, so it is found here; an unloaded fork reopened meanwhile
        // is not, and the store refuses the delete while it holds its file.
        let _unloading = self.unloading.read().await;
        let _reopening = self.reopening.lock(&id).await;
        let forks: Vec<SessionId> = self
            .loaded()
            .values()
            .filter(|session| session.fork_of.as_ref() == Some(&id))
            .map(|session| session.id.clone())
            .collect();
        let mut _reopening_forks = Vec::with_capacity(forks.len());
        for fork in &forks {
            _reopening_forks.push(self.reopening.lock(fork).await);
        }
        let loaded = self.loaded().get(&id).cloned();
        if loaded.is_none() {
            if let SessionJob::Busy(job) = self.jobs.session_job(&id) {
                return Err(writing(&job, &id));
            }
        }
        // Each loaded session the delete reaches, its forks first, is held
        // and checked before any is let go, so a refused delete leaves them
        // all loaded.
        let mut taken = Vec::with_capacity(forks.len() + 1);
        for fork in forks {
            // Unloaded meanwhile, as by its idle unload.
            let Some(session) = self.loaded().get(&fork).cloned() else {
                continue;
            };
            let held = self.hold_to_delete(fork, session, params.unless_attached);
            taken.push(held.await.map_err(|error| in_fork(error, &id))?);
        }
        if let Some(session) = &loaded {
            let held = self.hold_to_delete(id.clone(), Arc::clone(session), params.unless_attached);
            taken.push(held.await?);
        }
        // The store's refusals, checked while the sessions are still loaded,
        // whose leases on their files it skips: only those, since another
        // process may hold the session's file while the daemon has it
        // unloaded.
        {
            let id = id.clone();
            let open: Vec<String> = taken.iter().map(|held| held.id.to_string()).collect();
            store_blocking("check the session's delete", move || {
                StoredSession::check_delete_exact(id.as_str(), &open)
            })
            .await?;
        }
        let unloaded = self.unload_to_delete(taken, params.unless_attached).await?;
        let deleted = {
            let id = id.clone();
            store_blocking("delete the session", move || {
                StoredSession::delete_exact(id.as_str())
            })
            .await
        };
        // The store refused after its check passed, as on an I/O error, with
        // the sessions unloaded: their clients attach again to reopen those
        // that saved anything.
        let detached = match deleted {
            Ok(_) => DetachReason::Deleted,
            Err(_) => DetachReason::Lagged,
        };
        for session in &unloaded {
            session.feed.close(detached);
        }
        let child_sessions = match deleted? {
            ExactDelete::Deleted(outcome) => outcome.deleted_child_session_count,
            // A session loaded before it saved anything has no files: it is
            // gone once unloaded.
            ExactDelete::NothingSaved if loaded.is_some() => 0,
            ExactDelete::NothingSaved => return Err(not_found(&id)),
        };
        // A T3 wake watcher's recorded wait, and the pull requests a session
        // linked, go with it and its forks.
        let forks = unloaded
            .iter()
            .map(|session| &session.id)
            .filter(|fork| **fork != id);
        for gone in std::iter::once(&id).chain(forks) {
            WakeSignal::forget(&self.wakes, gone.as_str());
            self.pull_requests.forget(gone);
        }
        tracing::info!(session = %id, child_sessions, "deleted session");
        Ok(SessionDeleteResponse {})
    }

    /// Holds `session`, loaded as `id`, for its delete and takes its
    /// runtime, unless a settings change is queued or applying, its turn or
    /// background work is under way, or `unless_attached` and a client
    /// follows it. Held from prompts and settings changes until the
    /// returned hold is dropped, which puts the runtime back unless the
    /// delete unloaded the session.
    async fn hold_to_delete(
        &self,
        id: SessionId,
        session: Arc<LoadedSession>,
        unless_attached: bool,
    ) -> Result<HeldToDelete, RpcError> {
        let held = {
            // Under the turn lock a prompt starts its turn under, so this
            // sees every turn started before it, and a prompt after it sees
            // the delete.
            let mut turn = session.turn();
            match turn.hold {
                Some(Hold::Configuring(_)) => return Err(busy(CHANGE_QUEUED)),
                // Deletes of one session wait on its reopen lock, so none
                // finds another under way.
                Some(Hold::Deleting) => return Err(being_deleted()),
                None => {}
            }
            if unless_attached && session.feed.followed() {
                return Err(open_in_a_client(&id));
            }
            match self.jobs.session_job(&id) {
                SessionJob::Busy(job) if job == session.job => return Err(busy(TURN_UNDER_WAY)),
                SessionJob::Busy(job) => return Err(writing(&job, &id)),
                SessionJob::New | SessionJob::Last(_) => {}
            }
            Held::delete(&session, &mut turn)
        };
        // Free once a turn that just recorded its end released it.
        let mut slot = Arc::clone(&session.runtime).lock_owned().await;
        // Only the daemon's stop takes the runtime of a session in use.
        let Some(runtime) = slot.take() else {
            return Err(stopping());
        };
        if runtime.has_background_work() {
            *slot = Some(runtime);
            return Err(busy(BACKGROUND_WORK));
        }
        Ok(HeldToDelete {
            id,
            session,
            slot,
            runtime: Some(runtime),
            held: Some(held),
        })
    }

    /// Unloads the sessions in `taken`, held for a delete whose checks
    /// passed, all or none: takes them out of `loaded` and tears their
    /// runtimes, which held their files, down. None when a client attached
    /// to one meanwhile and `unless_attached`, or the daemon began to stop.
    async fn unload_to_delete(
        &self,
        mut taken: Vec<HeldToDelete>,
        unless_attached: bool,
    ) -> Result<Vec<Arc<LoadedSession>>, RpcError> {
        {
            // Again, under the locks that evict: a client that attached
            // while the checks waited is not detached by the delete, and one
            // that attaches from here finds the session gone. No other
            // request holds two turn locks, so taking them all cannot
            // deadlock.
            let mut turns: Vec<_> = taken.iter().map(|held| held.session.turn()).collect();
            if unless_attached {
                if let Some(held) = taken.iter().find(|held| held.session.feed.followed()) {
                    return Err(open_in_a_client(&held.id));
                }
            }
            let mut evicting: Vec<_> = taken
                .iter()
                .zip(turns.iter_mut())
                .map(|(held, turn)| (&held.id, &held.session, &mut **turn))
                .collect();
            // Only the stop takes a session out of `loaded` while its
            // reopen lock is held, as each is here.
            if !self.evict(&mut evicting) {
                return Err(stopping());
            }
        }
        let mut sessions = Vec::with_capacity(taken.len());
        for held in &mut taken {
            let (session, runtime, hold) = held.unloaded();
            runtime.shutdown().await;
            drop(hold);
            sessions.push(session);
        }
        Ok(sessions)
    }
}

/// A loaded session held for its delete, with its runtime taken: put back
/// when dropped, unless the delete unloaded the session.
struct HeldToDelete {
    id: SessionId,
    session: Arc<LoadedSession>,
    slot: OwnedMutexGuard<Option<SessionRuntime>>,
    runtime: Option<SessionRuntime>,
    held: Option<Held>,
}

impl HeldToDelete {
    /// The session, evicted, with its runtime and its hold, which its
    /// delete drops once the runtime is torn down.
    fn unloaded(&mut self) -> (Arc<LoadedSession>, SessionRuntime, Held) {
        let runtime = self.runtime.take().expect("taken until unloaded");
        let held = self.held.take().expect("held until unloaded");
        (Arc::clone(&self.session), runtime, held)
    }
}

impl Drop for HeldToDelete {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            *self.slot = Some(runtime);
        }
    }
}

/// `error`, which a fork of session `id` refused its delete with, said of
/// that fork: forks are not listed, so the person knows it as a side chat.
fn in_fork(error: RpcError, id: &SessionId) -> RpcError {
    RpcError {
        message: format!("a side chat forked from session {id}: {}", error.message),
        ..error
    }
}

const TURN_UNDER_WAY: &str = "the session has a turn in progress; delete it once the turn ends";
const CHANGE_QUEUED: &str =
    "a change to the session's settings is queued or applying; delete it once the change has applied";
const BACKGROUND_WORK: &str =
    "background work the session started, such as a subagent, is still running; delete it once that ends";

/// `limit` as a page size: [`catalog::DEFAULT_PAGE_SIZE`] when absent, and
/// refused over [`MAX_PAGE_SIZE`].
fn page_size(limit: Option<NonZeroU32>) -> Result<NonZeroUsize, RpcError> {
    let Some(limit) = limit else {
        return Ok(catalog::DEFAULT_PAGE_SIZE);
    };
    NonZeroUsize::try_from(limit)
        .ok()
        .filter(|size| size.get() <= MAX_PAGE_SIZE)
        .ok_or_else(|| {
            invalid(format!(
                "limit {limit} is over the session list's page limit (MAX_PAGE_SIZE = {MAX_PAGE_SIZE})"
            ))
        })
}

/// `summary` as `rho/session/list` sends it.
fn summary_to_wire(summary: SessionSummary) -> wire::SessionSummary {
    let SessionSummary {
        id,
        path: _,
        cwd,
        created_at: _,
        updated_at,
        message_count: _,
        title,
        first_user_message,
        last_user_message,
        added_dirs,
        origin: _,
    } = summary;
    wire::SessionSummary {
        session: wire::SessionId::new(id),
        cwd,
        added_dirs,
        title,
        first_user_message: first_user_message.as_deref().and_then(catalog::preview),
        last_user_message: last_user_message.as_deref().and_then(catalog::preview),
        updated_at,
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

/// The refusal of a delete that spares a session a client follows.
fn open_in_a_client(id: &SessionId) -> RpcError {
    RpcError::new(
        ErrorCode::SessionInUse,
        format!("session {id} is open in a client; close it there first"),
    )
}

fn busy(message: &str) -> RpcError {
    RpcError::new(ErrorCode::SessionBusy, message)
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
