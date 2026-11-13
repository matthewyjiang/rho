//! The catalog of saved sessions a person can resume, paged newest first:
//! the one query behind ACP's `session/list` and the daemon's
//! `rho/session/list` (#1670, #1667). Callers list a page with [`list`] and
//! map its summaries to their own wire.

use std::{borrow::Cow, cmp::Ordering, num::NonZeroUsize, path::Path};

use super::{session_root, Session, SessionListing, SessionSummary};

/// Sessions per page unless a caller asks for another size. Zed's thread
/// import walks every page, and each page reads the whole session index
/// again. Listing 445 saved sessions across all workspaces took about 0.1 s
/// and 80 KB of ACP `SessionInfo` JSON on a dev machine, so a page is about
/// 18 KB and a large store still lists in a handful of requests. The
/// daemon's summaries are larger, 393 B at most each on a dev machine's
/// store, so its default page is about 40 KB.
pub(crate) const DEFAULT_PAGE_SIZE: NonZeroUsize = NonZeroUsize::new(100).unwrap();

/// How many characters of a user message a listing shows: the `/resume`
/// picker's preview length.
const PREVIEW_CHARS: usize = 80;

/// One page of the catalog, and the cursor of the next, if there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogPage {
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) next_cursor: Option<String>,
}

/// Why a listing failed: the first two are the caller's request, the last
/// the store's.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CatalogError {
    #[error("cwd must be an absolute path")]
    RelativeCwd,
    /// A cursor no listing issued.
    #[error("unknown session list cursor '{0}'")]
    UnknownCursor(String),
    #[error(transparent)]
    Store(anyhow::Error),
}

/// The page of at most `limit` person-facing sessions after `cursor`, a
/// page's `next_cursor`, newest first: those saved in workspace `cwd`, an
/// absolute path, or in every workspace without one. `cwd` is canonicalized
/// as a new session's workspace is, so a symlink to a workspace lists its
/// sessions; one that does not resolve to a directory lists none. Blocking:
/// reads SQLite and walks session directories.
pub(crate) fn list(
    cwd: Option<&Path>,
    cursor: Option<&str>,
    limit: NonZeroUsize,
) -> Result<CatalogPage, CatalogError> {
    let root = session_root().map_err(CatalogError::Store)?;
    list_in_root(&root, cwd, cursor, limit)
}

fn list_in_root(
    root: &Path,
    cwd: Option<&Path>,
    cursor: Option<&str>,
    limit: NonZeroUsize,
) -> Result<CatalogPage, CatalogError> {
    if cwd.is_some_and(|cwd| !cwd.is_absolute()) {
        return Err(CatalogError::RelativeCwd);
    }
    let after = cursor
        .map(|cursor| {
            Cursor::parse(cursor).ok_or_else(|| CatalogError::UnknownCursor(cursor.into()))
        })
        .transpose()?;
    let summaries = match cwd {
        // In the form a session's workspace is saved under.
        Some(cwd) => match crate::paths::canonicalize(cwd) {
            // Checked before listing, which creates the workspace's session
            // directory; a session in a missing workspace cannot load anyway.
            Ok(cwd) if cwd.is_dir() => {
                Session::list_in_root(root, &cwd, SessionListing::PersonFacing)
            }
            Ok(_) | Err(_) => Ok(Vec::new()),
        },
        None => Session::list_all_in_root(root, SessionListing::PersonFacing),
    }
    .map_err(CatalogError::Store)?;
    Ok(page(summaries, after.as_ref(), limit))
}

/// The page of at most `limit` of `summaries` after `after`, newest first.
/// Sessions `/resume` hides, those with nothing saved yet, are left out, as
/// are sessions whose workspace directory is gone, which neither ACP's
/// `session/load` nor the daemon's attach reopens.
fn page(
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
    for summary in &mut summaries {
        // Like the workspace, only the added directories still there: an
        // attach restores them, and ACP's `session/load` refuses one that is
        // gone.
        summary.added_dirs.retain(|dir| dir.is_dir());
    }
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
/// from repeating entries, or skipping one deleted between pages; a session
/// saved during the walk moves ahead of the cursor and is missed.
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

    /// The cursor a page's `next_cursor` encoded, or `None` for a string no
    /// listing issued.
    fn parse(cursor: &str) -> Option<Cursor<'static>> {
        serde_json::from_str::<(u64, String, String)>(cursor)
            .ok()
            .map(|(updated_at, id, cwd)| Cursor {
                updated_at,
                id: Cow::Owned(id),
                cwd: Cow::Owned(cwd),
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

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
