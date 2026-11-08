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
    catalog::{self, CatalogScope, Cursor},
    DeleteOptions, Session as StoredSession, SessionListing, SessionSummary, SessionTarget,
};

/// Lists saved sessions newest first: the request's workspace, or every
/// workspace when it names none.
pub(super) async fn list(request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpError> {
    off_runtime(move || {
        let scope = match &request.cwd {
            Some(cwd) if !cwd.is_absolute() => {
                return Err(AcpError::invalid_params().data("cwd must be an absolute path"));
            }
            Some(cwd) => CatalogScope::Workspace(cwd),
            None => CatalogScope::Everywhere,
        };
        let after = request
            .cursor
            .as_deref()
            .map(Cursor::parse)
            .transpose()
            .map_err(|error| AcpError::invalid_params().data(error.to_string()))?;
        let summaries = catalog::resumable(scope).map_err(store_error)?;
        let page = catalog::page(summaries, after.as_ref(), catalog::DEFAULT_PAGE_SIZE);
        Ok(
            ListSessionsResponse::new(page.sessions.into_iter().map(session_info).collect())
                .next_cursor(page.next_cursor),
        )
    })
    .await
}

/// Deletes saved session `session_id` the way `rho sessions rm` does. ACP
/// deletes are idempotent, so an id with nothing saved, never created or
/// already deleted, succeeds. The store refuses a session another Rho
/// process has open and one whose delegated runs are still going.
pub(super) async fn delete(session_id: SessionId) -> Result<DeleteSessionResponse, AcpError> {
    off_runtime(move || {
        if let Some(target) = saved_target(session_id.0.as_ref())? {
            StoredSession::delete_target(&target, DeleteOptions::default()).map_err(store_error)?;
        }
        Ok(DeleteSessionResponse::new())
    })
    .await
}

/// The one saved session, in any workspace, whose id is exactly `id`. Hosts
/// name whole ids, and ids need not be UUIDs, so a store prefix lookup could
/// match another session or stop at a longer id in one workspace.
fn saved_target(id: &str) -> Result<Option<SessionTarget>, AcpError> {
    let mut matches = StoredSession::list_all(SessionListing::Everything)
        .map_err(store_error)?
        .into_iter()
        .filter(|summary| summary.id == id);
    match (matches.next(), matches.next()) {
        (Some(_), Some(_)) => Err(AcpError::invalid_request().data(format!(
            "session '{id}' is saved in more than one workspace"
        ))),
        (first, _) => Ok(first.as_ref().map(SessionSummary::target)),
    }
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
    // `session/load` takes roots only from its request and refuses one that
    // is gone, so a host that sends back what is listed must not get a
    // deleted directory.
    let additional_directories = summary
        .added_dirs
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect();
    SessionInfo::new(SessionId::new(summary.id), summary.cwd)
        .additional_directories(additional_directories)
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

/// A session something else holds is the caller's to retry once it is
/// closed; every other store failure is internal.
fn store_error(error: anyhow::Error) -> AcpError {
    if error.is::<crate::session::SessionInUse>() {
        return AcpError::invalid_request().data(error.to_string());
    }
    AcpError::internal_error().data(error.to_string())
}

#[cfg(test)]
#[path = "session_list_tests.rs"]
mod tests;
