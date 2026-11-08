//! The session catalog over the socket (#1670, #1667): `rho/session/list`,
//! `rho/session/rename`, and `rho/session/delete`, over the session store's
//! catalog, titles, and delete, which ACP's `session/list` and
//! `session/delete` and `rho sessions` use too.
//!
//! A list adds, to what the store saved, the job state of each session the
//! daemon has loaded. A rename tells a loaded session's attached clients
//! the new title. A delete must unload a loaded session first, since its
//! runtime holds the file's lease: it refuses one whose turn, settings
//! change, or background work is under way, holds it from prompts while it
//! tears it down, and then detaches its clients as `deleted`, or as
//! `unloaded` when the store refused the delete after all.

use std::{num::NonZeroU32, num::NonZeroUsize, sync::Arc};

use rho_protocol::{
    self as wire, DetachReason, ErrorCode, RpcError, SessionDeleteParams, SessionDeleteResponse,
    SessionListParams, SessionListResponse, SessionRenameParams, SessionRenameResponse,
};
use rho_sdk::SessionId;

use super::{
    being_deleted,
    reopen::{not_found, parse_session, saved_workspace},
    stopping,
    unload::Residency,
    writing, Held, Hold, LoadedSession, Sessions,
};
use crate::{
    app::daemon_host::jobs::SessionJob,
    session::{
        catalog::{self, CatalogScope, Cursor},
        DeleteOptions, Session as StoredSession, SessionInUse, SessionSummary, SessionTarget,
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
    /// One page of the saved sessions a person can resume, newest first,
    /// each with its job state when the daemon has it loaded.
    pub(in crate::app::daemon_host) async fn list(
        &self,
        params: SessionListParams,
    ) -> Result<SessionListResponse, RpcError> {
        let SessionListParams { cwd, cursor, limit } = params;
        let limit = page_size(limit)?;
        if cwd.as_ref().is_some_and(|cwd| !cwd.is_absolute()) {
            return Err(invalid("cwd must be an absolute path"));
        }
        let after = cursor
            .as_deref()
            .map(Cursor::parse)
            .transpose()
            .map_err(|error| invalid(error.to_string()))?;
        let listed = tokio::task::spawn_blocking(move || {
            let scope = cwd
                .as_deref()
                .map_or(CatalogScope::Everywhere, CatalogScope::Workspace);
            catalog::resumable(scope)
                .map(|summaries| catalog::page(summaries, after.as_ref(), limit))
        })
        .await;
        let page = match listed {
            Ok(Ok(page)) => page,
            Ok(Err(error)) => return Err(store_failed("list the sessions", &error)),
            Err(error) => return Err(store_failed("list the sessions", &error.into())),
        };
        let loaded = self.loaded().clone();
        let sessions = page
            .sessions
            .into_iter()
            .map(|summary| {
                let state = SessionId::from_string(summary.id.as_str())
                    .ok()
                    .and_then(|id| loaded.get(&id))
                    .map(|session| session.feed.job_state());
                summary_to_wire(summary, state)
            })
            .collect();
        Ok(SessionListResponse {
            sessions,
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
        let title = params.title.trim().to_owned();
        if title.is_empty() {
            return Err(invalid("a session's title must not be empty"));
        }
        let cwd = saved_workspace(&id).await?;
        let renamed = {
            let id = id.clone();
            tokio::task::spawn_blocking(move || StoredSession::set_title(&cwd, id.as_str(), &title))
                .await
        };
        let title = match renamed {
            Ok(Ok(update)) => update.title,
            Ok(Err(error)) => return Err(store_failed("rename the session", &error)),
            Err(error) => return Err(store_failed("rename the session", &error.into())),
        };
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
            Some(session) => self.unload_to_delete(&id, session).await?,
            None => {
                if let SessionJob::Busy(job) = self.jobs.session_job(&id) {
                    return Err(writing(&job, &id));
                }
            }
        }
        match (loaded, delete_saved(&id).await) {
            (Some(session), Ok(_)) => session.feed.close(DetachReason::Deleted),
            (Some(session), Err(error)) => {
                session.feed.close(DetachReason::Unloaded);
                return Err(error);
            }
            (None, Ok(Saved::Deleted)) => {}
            (None, Ok(Saved::Nothing)) => return Err(not_found(&id)),
            (None, Err(error)) => return Err(error),
        }
        tracing::info!(session = %id, "deleted session");
        Ok(SessionDeleteResponse {})
    }

    /// Unloads `session`, loaded as `id`, for its delete, unless its turn, a
    /// settings change, or its background work is under way. Held from
    /// prompts and settings changes meanwhile; once this returns, the
    /// session is out of `loaded` and its runtime, which held its file, torn
    /// down.
    async fn unload_to_delete(
        &self,
        id: &SessionId,
        session: &Arc<LoadedSession>,
    ) -> Result<(), RpcError> {
        let held = {
            // Under the turn lock a prompt starts its turn under, so this
            // sees every turn started before it, and a prompt after it sees
            // the delete.
            let mut turn = session.turn();
            match self.jobs.session_job(id) {
                SessionJob::Busy(job) if job == session.job => return Err(busy(TURN_UNDER_WAY)),
                SessionJob::Busy(job) => return Err(writing(&job, id)),
                SessionJob::New | SessionJob::Last(_) => {}
            }
            match turn.hold {
                Hold::Configuring => return Err(busy(CHANGE_APPLYING)),
                // Deletes of one session wait on its reopen lock, so none
                // finds another under way.
                Hold::Deleting => return Err(being_deleted()),
                Hold::Free => turn.hold = Hold::Deleting,
            }
            Held(Arc::clone(session))
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
        {
            // Lock order: the turn lock, then `loaded`. Under the lock
            // `begin_stop` takes, so a stop either drains the session first,
            // and gets its runtime back, or finds it gone.
            let mut turn = session.turn();
            let mut loaded = self.loaded();
            if self.stopping.is_cancelled() {
                *slot = Some(runtime);
                return Err(stopping());
            }
            if loaded
                .get(id)
                .is_some_and(|current| Arc::ptr_eq(current, session))
            {
                loaded.remove(id);
            }
            turn.residency = Residency::Unloaded;
        }
        drop(slot);
        runtime.shutdown().await;
        drop(held);
        Ok(())
    }
}

const TURN_UNDER_WAY: &str = "the session has a turn in progress; delete it once the turn ends";
const CHANGE_APPLYING: &str =
    "a change to the session's settings is applying; delete it once the change has applied";
const BACKGROUND_WORK: &str =
    "background work the session started, such as a subagent, is still running; delete it once that ends";

/// What a delete found saved under a session's id.
enum Saved {
    Deleted,
    Nothing,
}

/// Deletes the files of the session saved under `id`, with the sessions it
/// started, as `rho sessions rm` does.
async fn delete_saved(id: &SessionId) -> Result<Saved, RpcError> {
    let id = id.clone();
    let deleted = tokio::task::spawn_blocking(move || -> anyhow::Result<Saved> {
        let Some(cwd) = StoredSession::workspace_of(id.as_str())? else {
            return Ok(Saved::Nothing);
        };
        let target = SessionTarget::new(id.as_str(), cwd);
        StoredSession::delete_target(&target, DeleteOptions::default())?;
        Ok(Saved::Deleted)
    })
    .await;
    match deleted {
        Ok(Ok(saved)) => Ok(saved),
        Ok(Err(error)) if error.is::<SessionInUse>() => {
            Err(RpcError::new(ErrorCode::SessionInUse, format!("{error:#}")))
        }
        Ok(Err(error)) => Err(store_failed("delete the session", &error)),
        Err(error) => Err(store_failed("delete the session", &error.into())),
    }
}

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

/// `summary` as `rho/session/list` sends it, with the job `state` of a
/// loaded session.
fn summary_to_wire(summary: SessionSummary, state: Option<wire::JobState>) -> wire::SessionSummary {
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
        // An attach restores them; a client shows only those still there.
        added_dirs: added_dirs.into_iter().filter(|dir| dir.is_dir()).collect(),
        title,
        first_user_message: first_user_message.as_deref().and_then(catalog::preview),
        last_user_message: last_user_message.as_deref().and_then(catalog::preview),
        updated_at,
        state,
    }
}

fn store_failed(what: &str, error: &anyhow::Error) -> RpcError {
    RpcError::new(
        ErrorCode::InternalError,
        format!("could not {what}: {error:#}"),
    )
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

fn busy(message: &str) -> RpcError {
    RpcError::new(ErrorCode::SessionBusy, message)
}
