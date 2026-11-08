//! The catalog of saved sessions a person can resume, paged newest first:
//! the one query behind ACP's `session/list` and the daemon's
//! `rho/session/list` (#1670, #1667). Callers parse their request's cursor
//! with [`Cursor::parse`], list with [`resumable`], and cut a page with
//! [`page`]; each maps the summaries to its own wire.

use std::{borrow::Cow, cmp::Ordering, num::NonZeroUsize, path::Path};

use super::{Session, SessionListing, SessionSummary};

/// Sessions per page unless a caller asks for another size. Zed's thread
/// import walks every page, and each page reads the whole session index
/// again. Listing 445 saved sessions across all workspaces took about 0.1 s
/// and 80 KB of ACP `SessionInfo` JSON on a dev machine, so a page is about
/// 18 KB and a large store still lists in a handful of requests.
pub(crate) const DEFAULT_PAGE_SIZE: NonZeroUsize = NonZeroUsize::new(100).unwrap();

/// How many characters of a user message a listing shows: the `/resume`
/// picker's preview length.
const PREVIEW_CHARS: usize = 80;

/// Which workspaces a listing covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogScope<'a> {
    Everywhere,
    /// Only the sessions saved in this workspace.
    Workspace(&'a Path),
}

/// One page of the catalog, and the cursor of the next, if there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogPage {
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) next_cursor: Option<String>,
}

/// A cursor no listing issued.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown session list cursor '{0}'")]
pub(crate) struct UnknownCursor(String);

/// The person-facing sessions saved in `scope`, unpaged. Blocking: reads
/// SQLite and walks session directories.
pub(crate) fn resumable(scope: CatalogScope<'_>) -> anyhow::Result<Vec<SessionSummary>> {
    match scope {
        // Listing creates the workspace's session directory, and a session
        // in a missing workspace cannot load anyway.
        CatalogScope::Workspace(cwd) if !cwd.is_dir() => Ok(Vec::new()),
        CatalogScope::Workspace(cwd) => Session::list(cwd, SessionListing::PersonFacing),
        CatalogScope::Everywhere => Session::list_all(SessionListing::PersonFacing),
    }
}

/// The page of at most `limit` of `summaries` after `after`, newest first.
/// Sessions `/resume` hides, those with nothing saved yet, are left out, as
/// are sessions whose workspace directory is gone, which neither ACP's
/// `session/load` nor the daemon's attach reopens.
pub(crate) fn page(
    mut summaries: Vec<SessionSummary>,
    after: Option<&Cursor<'_>>,
    limit: NonZeroUsize,
) -> CatalogPage {
    let limit = limit.get();
    summaries.retain(|summary| {
        summary.has_messages()
            && summary.cwd.is_dir()
            && after.is_none_or(|after| after.precedes(summary))
    });
    summaries.sort_by(|left, right| Cursor::at(left).cmp_list_order(&Cursor::at(right)));
    let next_cursor = summaries
        .get(limit)
        .map(|_| Cursor::at(&summaries[limit - 1]).encode());
    summaries.truncate(limit);
    CatalogPage {
        sessions: summaries,
        next_cursor,
    }
}

/// `message` on one line, cut to [`PREVIEW_CHARS`], or `None` when it has
/// no text: how a listing shows a user message, and labels an untitled
/// session with its first.
pub(crate) fn preview(message: &str) -> Option<String> {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= PREVIEW_CHARS {
        return Some(line);
    }
    let mut cut = line.chars().take(PREVIEW_CHARS - 1).collect::<String>();
    cut.push('…');
    Some(cut)
}

/// A session's place in list order: newest `updated_at` first, then id, then
/// workspace, which also orders one id saved in two workspaces. A cursor is
/// the place of the last session a page returned, and the next page starts
/// right after it. Keying on a session rather than an offset keeps a walk
/// from repeating or skipping entries when sessions are created or deleted
/// between pages.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Cursor<'a> {
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

    /// The cursor a page's `next_cursor` encoded.
    pub(crate) fn parse(cursor: &str) -> Result<Cursor<'static>, UnknownCursor> {
        serde_json::from_str::<(u64, String, String)>(cursor)
            .map(|(updated_at, id, cwd)| Cursor {
                updated_at,
                id: Cow::Owned(id),
                cwd: Cow::Owned(cwd),
            })
            .map_err(|_| UnknownCursor(cursor.to_owned()))
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

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
