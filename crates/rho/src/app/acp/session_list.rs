//! `session/list` and the store side of `session/delete`: saved Rho sessions
//! as the host's history. Every listed id loads through `session/load`. The
//! listing is the session catalog's, which the daemon's `rho/session/list`
//! shares; this module maps it to ACP.

use agent_client_protocol::{
    schema::v1::{
        DeleteSessionResponse, ListSessionsRequest, ListSessionsResponse, SessionId, SessionInfo,
    },
    Error as AcpError,
};

use crate::session::{
    catalog::{self, CatalogError},
    SavedInManyWorkspaces, Session as StoredSession, SessionInUse, SessionSummary,
};

/// Lists saved sessions newest first: the request's workspace, or every
/// workspace when it names none.
pub(super) async fn list(request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpError> {
    off_runtime(move || {
        let page = catalog::list(
            request.cwd.as_deref(),
            request.cursor.as_deref(),
            catalog::DEFAULT_PAGE_SIZE,
        )
        .map_err(|error| match error {
            CatalogError::RelativeCwd | CatalogError::UnknownCursor(_) => {
                AcpError::invalid_params().data(error.to_string())
            }
            CatalogError::Store(error) => store_error(error),
        })?;
        Ok(
            ListSessionsResponse::new(page.sessions.into_iter().map(session_info).collect())
                .next_cursor(page.next_cursor),
        )
    })
    .await
}

/// Deletes the session saved under exactly `session_id` the way `rho
/// sessions rm` does. ACP deletes are idempotent, so an id with nothing
/// saved, never created or already deleted, succeeds. The store refuses a
/// session another Rho process has open, one whose delegated runs are still
/// going, and an id saved in more than one workspace.
pub(super) async fn delete(session_id: SessionId) -> Result<DeleteSessionResponse, AcpError> {
    off_runtime(move || {
        StoredSession::delete_exact(session_id.0.as_ref()).map_err(store_error)?;
        Ok(DeleteSessionResponse::new())
    })
    .await
}

/// `summary` as ACP lists it: labelled with its title, else its first user
/// message, with its last save as an RFC 3339 time.
fn session_info(summary: SessionSummary) -> SessionInfo {
    let title = summary.title.or_else(|| {
        summary
            .first_user_message
            .as_deref()
            .and_then(catalog::preview)
    });
    let updated_at = i64::try_from(summary.updated_at)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, /*use_z*/ true));
    // Only those still there, which the catalog keeps: `session/load` takes
    // roots only from its request and refuses one that is gone.
    SessionInfo::new(SessionId::new(summary.id), summary.cwd)
        .additional_directories(summary.added_dirs)
        .title(title)
        .updated_at(updated_at)
}

/// Runs blocking session-store work off the async runtime: the store reads
/// SQLite and walks session directories.
async fn off_runtime<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AcpError> + Send + 'static,
) -> Result<T, AcpError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| AcpError::internal_error().data(error.to_string()))?
}

/// A session something else holds, its own or a child session's, is the
/// caller's to retry once it is closed, and an id saved in two workspaces
/// names none; every other store failure is internal.
fn store_error(error: anyhow::Error) -> AcpError {
    let refused = error
        .chain()
        .any(|cause| cause.is::<SessionInUse>() || cause.is::<SavedInManyWorkspaces>());
    if refused {
        return AcpError::invalid_request().data(format!("{error:#}"));
    }
    AcpError::internal_error().data(format!("{error:#}"))
}

#[cfg(test)]
#[path = "session_list_tests.rs"]
mod tests;
