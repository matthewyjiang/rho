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
//! then detaches its clients as `deleted`.

use std::{num::NonZeroU32, num::NonZeroUsize, sync::Arc};

use rho_protocol::{
    self as wire, DetachReason, ErrorCode, RpcError, SessionDeleteParams, SessionDeleteResponse,
    SessionListParams, SessionListResponse, SessionRenameParams, SessionRenameResponse,
};
use rho_sdk::SessionId;

use super::{
    being_deleted,
    reopen::{not_found, parse_session},
    stopping,
    store::{store_blocking, store_failed},
    t3_wake::WakeSignal,
    writing, Held, Hold, LoadedSession, Sessions,
};
use crate::{
    app::daemon_host::jobs::SessionJob,
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
    /// first when the daemon has it loaded.
    pub(in crate::app::daemon_host) async fn delete(
        &self,
        params: SessionDeleteParams,
    ) -> Result<SessionDeleteResponse, RpcError> {
        self.refuse_when_stopping()?;
        let id = parse_session(&params.session)?;
        // Both held until the files are gone: the stop waits for the first,
        // and a request that would reopen the session waits for the second,
        // then finds nothing saved.
        let _unloading = self.unloading.read().await;
        let _reopening = self.reopening.lock(&id).await;
        let loaded = self.loaded().get(&id).cloned();
        match &loaded {
            Some(session) => {
                self.unload_to_delete(&id, session, params.unless_attached)
                    .await?;
            }
            None => {
                if let SessionJob::Busy(job) = self.jobs.session_job(&id) {
                    return Err(writing(&job, &id));
                }
            }
        }
        let deleted = {
            let id = id.clone();
            store_blocking("delete the session", move || {
                StoredSession::delete_exact(id.as_str())
            })
            .await
        };
        if deleted.is_ok() {
            // A T3 wake watcher's recorded wait goes with its session.
            WakeSignal::forget(&self.wakes, id.as_str());
        }
        let child_sessions = match (loaded, deleted) {
            (Some(session), Ok(ExactDelete::Deleted(outcome))) => {
                session.feed.close(DetachReason::Deleted);
                outcome.deleted_child_session_count
            }
            // A session loaded before it saved anything has no files: it is
            // gone once unloaded.
            (Some(session), Ok(ExactDelete::NothingSaved)) => {
                session.feed.close(DetachReason::Deleted);
                0
            }
            // The store refused after its check passed, as on an I/O error,
            // with the session unloaded: its clients attach again to reopen
            // it.
            (Some(session), Err(error)) => {
                session.feed.close(DetachReason::Lagged);
                return Err(error);
            }
            (None, Ok(ExactDelete::Deleted(outcome))) => outcome.deleted_child_session_count,
            (None, Ok(ExactDelete::NothingSaved)) => return Err(not_found(&id)),
            (None, Err(error)) => return Err(error),
        };
        tracing::info!(session = %id, child_sessions, "deleted session");
        Ok(SessionDeleteResponse {})
    }

    /// Unloads `session`, loaded as `id`, for its delete, unless a settings
    /// change is queued or applying, its turn or background work is under
    /// way, or the store would refuse the delete. Held from prompts and
    /// settings changes meanwhile; once this returns, the session is out of
    /// `loaded` and its runtime, which held its file, torn down.
    async fn unload_to_delete(
        &self,
        id: &SessionId,
        session: &Arc<LoadedSession>,
        unless_attached: bool,
    ) -> Result<(), RpcError> {
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
                return Err(RpcError::new(
                    ErrorCode::SessionInUse,
                    format!("session {id} is open in a client; close it there first"),
                ));
            }
            match self.jobs.session_job(id) {
                SessionJob::Busy(job) if job == session.job => return Err(busy(TURN_UNDER_WAY)),
                SessionJob::Busy(job) => return Err(writing(&job, id)),
                SessionJob::New | SessionJob::Last(_) => {}
            }
            Held::delete(session, &mut turn)
        };
        // Free once a turn that just recorded its end released it.
        let mut slot = session.runtime.lock().await;
        // Only the daemon's stop takes the runtime of a session in use.
        let Some(runtime) = slot.take() else {
            return Err(stopping());
        };
        if runtime.has_background_work() {
            *slot = Some(runtime);
            return Err(busy(BACKGROUND_WORK));
        }
        // The store's refusals, checked while the session is still loaded:
        // a delete it refuses leaves the session as it was.
        let checked = {
            let id = id.clone();
            store_blocking("check the session's delete", move || {
                StoredSession::check_delete_exact(id.as_str())
            })
            .await
        };
        if let Err(error) = checked {
            *slot = Some(runtime);
            return Err(error);
        }
        // Only the stop takes a session out of `loaded` while its reopen
        // lock is held, as it is here.
        if !self.evict(id, session, &mut session.turn()) {
            *slot = Some(runtime);
            return Err(stopping());
        }
        drop(slot);
        runtime.shutdown().await;
        drop(held);
        Ok(())
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

fn busy(message: &str) -> RpcError {
    RpcError::new(ErrorCode::SessionBusy, message)
}
