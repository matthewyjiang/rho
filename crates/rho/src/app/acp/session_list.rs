//! `session/list` and the store side of `session/delete`: saved Rho sessions
//! as the host's history. Every listed id loads through `session/load`.

use std::{borrow::Cow, cmp::Ordering};

use agent_client_protocol::{
    schema::v1::{
        DeleteSessionResponse, ListSessionsRequest, ListSessionsResponse, SessionId, SessionInfo,
    },
    Error as AcpError,
};

use crate::session::{DeleteOptions, Session as StoredSession, SessionSummary, SessionTarget};

/// Sessions per `session/list` page. Zed's thread import walks every page,
/// and each page reads the whole session index again. Listing 445 saved
/// sessions across all workspaces took about 0.1 s and 80 KB of
/// `SessionInfo` JSON on a dev machine, so a page is about 18 KB and a large
/// store still lists in a handful of requests.
const PAGE_SIZE: usize = 100;

/// An untitled session is labeled with its first user message, cut to the
/// `/resume` picker's preview length.
const FALLBACK_TITLE_CHARS: usize = 80;

/// Lists saved sessions newest first: the request's workspace, or every
/// workspace when it names none.
pub(super) async fn list(request: ListSessionsRequest) -> Result<ListSessionsResponse, AcpError> {
    off_runtime(move || {
        let summaries = match &request.cwd {
            Some(cwd) if !cwd.is_absolute() => {
                return Err(AcpError::invalid_params().data("cwd must be an absolute path"));
            }
            // Listing creates the workspace's session directory, and a
            // session in a missing workspace cannot load anyway.
            Some(cwd) if !cwd.is_dir() => Vec::new(),
            Some(cwd) => StoredSession::list(cwd).map_err(store_error)?,
            None => StoredSession::list_all().map_err(store_error)?,
        };
        page(summaries, request.cursor.as_deref())
    })
    .await
}

/// The stored session `session/delete` names, resolved before anything open
/// is closed, so an unknown or ambiguous id leaves the connection untouched.
pub(super) async fn delete_target(session_id: SessionId) -> Result<SessionTarget, AcpError> {
    off_runtime(move || Ok(exact_session(session_id.0.as_ref())?.target())).await
}

/// Deletes saved session `target` the way `rho sessions rm` does. The store
/// refuses a session another Rho process has open and one whose delegated
/// runs are still going.
pub(super) async fn delete(target: SessionTarget) -> Result<DeleteSessionResponse, AcpError> {
    off_runtime(move || {
        StoredSession::delete_target(&target, DeleteOptions::default()).map_err(store_error)?;
        Ok(DeleteSessionResponse::new())
    })
    .await
}

/// The one saved session, in any workspace, whose id is exactly `id`. Hosts
/// name whole ids, and ids need not be UUIDs, so a store prefix lookup could
/// match another session or stop at a longer id in one workspace.
fn exact_session(id: &str) -> Result<SessionSummary, AcpError> {
    let mut matches = StoredSession::list_all()
        .map_err(store_error)?
        .into_iter()
        .filter(|summary| summary.id == id);
    match (matches.next(), matches.next()) {
        (Some(summary), None) => Ok(summary),
        (None, _) => Err(AcpError::resource_not_found(Some(id.to_owned()))),
        (Some(_), Some(_)) => Err(AcpError::invalid_request().data(format!(
            "session '{id}' is saved in more than one workspace"
        ))),
    }
}

/// The page of `summaries` after `cursor`. Sessions `/resume` hides, those
/// with nothing saved yet, are left out, as are sessions whose workspace
/// directory is gone, which `session/load` refuses.
fn page(
    mut summaries: Vec<SessionSummary>,
    cursor: Option<&str>,
) -> Result<ListSessionsResponse, AcpError> {
    let after = cursor.map(Cursor::parse).transpose()?;
    summaries.retain(|summary| {
        summary.has_messages()
            && summary.cwd.is_dir()
            && after.as_ref().is_none_or(|after| after.precedes(summary))
    });
    summaries.sort_by(|left, right| Cursor::at(left).cmp_list_order(&Cursor::at(right)));
    let next_cursor = summaries
        .get(PAGE_SIZE)
        .map(|_| Cursor::at(&summaries[PAGE_SIZE - 1]).encode());
    summaries.truncate(PAGE_SIZE);
    Ok(
        ListSessionsResponse::new(summaries.into_iter().map(session_info).collect())
            .next_cursor(next_cursor),
    )
}

/// A session's place in list order: newest `updated_at` first, then id, then
/// workspace, which also orders one id saved in two workspaces. A cursor is
/// the place of the last session a page returned, and the next page starts
/// right after it. Keying on a session rather than an offset keeps a walk
/// from repeating or skipping entries when sessions are created or deleted
/// between pages.
#[derive(Debug, PartialEq, Eq)]
struct Cursor<'a> {
    updated_at: u64,
    id: Cow<'a, str>,
    cwd: Cow<'a, str>,
}

impl<'a> Cursor<'a> {
    fn at(summary: &'a SessionSummary) -> Self {
        Self {
            updated_at: summary.updated_at,
            id: Cow::Borrowed(&summary.id),
            cwd: summary.cwd.to_string_lossy(),
        }
    }

    fn encode(&self) -> String {
        serde_json::json!([self.updated_at, self.id, self.cwd]).to_string()
    }

    fn parse(cursor: &str) -> Result<Cursor<'static>, AcpError> {
        serde_json::from_str::<(u64, String, String)>(cursor)
            .map(|(updated_at, id, cwd)| Cursor {
                updated_at,
                id: Cow::Owned(id),
                cwd: Cow::Owned(cwd),
            })
            .map_err(|_| {
                AcpError::invalid_params().data(format!("unknown session/list cursor '{cursor}'"))
            })
    }

    fn cmp_list_order(&self, other: &Cursor<'_>) -> Ordering {
        other
            .updated_at
            .cmp(&self.updated_at)
            .then_with(|| self.id.cmp(&other.id))
            .then_with(|| self.cwd.cmp(&other.cwd))
    }

    fn precedes(&self, summary: &SessionSummary) -> bool {
        self.cmp_list_order(&Cursor::at(summary)) == Ordering::Less
    }
}

fn session_info(summary: SessionSummary) -> SessionInfo {
    let title = summary.title.or_else(|| {
        summary
            .first_user_message
            .as_deref()
            .and_then(fallback_title)
    });
    let updated_at = i64::try_from(summary.updated_at)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, /*use_z*/ true));
    SessionInfo::new(SessionId::new(summary.id), summary.cwd)
        .title(title)
        .updated_at(updated_at)
}

/// `message` on one line, cut to [`FALLBACK_TITLE_CHARS`].
fn fallback_title(message: &str) -> Option<String> {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= FALLBACK_TITLE_CHARS {
        return Some(line);
    }
    let mut cut = line
        .chars()
        .take(FALLBACK_TITLE_CHARS - 1)
        .collect::<String>();
    cut.push('…');
    Some(cut)
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

fn store_error(error: anyhow::Error) -> AcpError {
    AcpError::internal_error().data(error.to_string())
}

#[cfg(test)]
#[path = "session_list_tests.rs"]
mod tests;
