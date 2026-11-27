//! One page of the catalog's sessions whose conversation matches a query:
//! the query behind the daemon's `rho/session/search`. The sessions are the
//! ones [`super::catalog`] lists, so every hit opens; their evidence is the
//! search index's ([`super::search_index`]), which callers refresh first.

use std::{cmp::Ordering, collections::HashMap, num::NonZeroUsize, path::Path};

use rho_protocol::{ExcerptRole, MessageId, SearchExcerpt, SearchLocation, ToolCallId};
use rusqlite::{params, Connection};

use super::{
    catalog::{self, CatalogError},
    search::{fts_query, LastTerm},
    search_window::window,
    SessionSummary,
};

/// The longest query a search takes, in characters: a tripwire for a
/// pasted log, not a cap on what a person types. Every word is a posting
/// list the index reads, so cost grows with length: 1000 characters of a
/// real prompt (about 160 words) took 150-330 ms on a dev machine's 450 MB
/// index, where one common word takes 10-70 ms.
pub(crate) const MAX_QUERY_CHARS: usize = 1000;

/// A search, as [`search`] takes it.
pub(crate) struct SearchRequest<'a> {
    pub(crate) query: &'a str,
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) cursor: Option<&'a str>,
    pub(crate) limit: NonZeroUsize,
}

/// One page of matching sessions, newest first.
#[derive(Debug)]
pub(crate) struct SearchPage {
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) next_cursor: Option<String>,
    /// How many sessions match in all.
    pub(crate) total: u64,
}

/// A matching session, with its best one or two matching messages.
#[derive(Debug)]
pub(crate) struct SearchHit {
    pub(crate) summary: SessionSummary,
    pub(crate) matching_messages: u64,
    pub(crate) excerpts: Vec<SearchExcerpt>,
}

/// Why a search failed: all but the last are the caller's request.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SearchError {
    #[error("query must hold a word")]
    BlankQuery,
    #[error(
        "query is over the search's limit (MAX_QUERY_CHARS = {MAX_QUERY_CHARS}): {0} characters"
    )]
    LongQuery(usize),
    #[error("cwd must be an absolute path")]
    RelativeCwd,
    /// A cursor no search issued.
    #[error("unknown session search cursor '{0}'")]
    UnknownCursor(String),
    #[error(transparent)]
    Store(anyhow::Error),
}

/// The page of at most `limit` sessions in session root `root` after
/// `cursor` whose evidence in the search index `connection` holds every word
/// of `query`, the last as a prefix too. Blocking: reads SQLite and walks
/// session directories.
pub(crate) fn search(
    connection: &Connection,
    root: &Path,
    request: SearchRequest<'_>,
) -> Result<SearchPage, SearchError> {
    let SearchRequest {
        query,
        cwd,
        cursor,
        limit,
    } = request;
    check_query(query)?;
    let after = cursor
        .map(|cursor| {
            Cursor::parse(cursor).ok_or_else(|| SearchError::UnknownCursor(cursor.into()))
        })
        .transpose()?;
    let mut listed: HashMap<String, SessionSummary> = catalog::listable_in_root(root, cwd)
        .map_err(|error| match error {
            CatalogError::RelativeCwd => SearchError::RelativeCwd,
            CatalogError::UnknownCursor(_) | CatalogError::Store(_) => {
                SearchError::Store(error.into())
            }
        })?
        .into_iter()
        .map(|summary| (summary.path.to_string_lossy().into_owned(), summary))
        .collect();
    let query = fts_query(query, LastTerm::Prefix);
    let store = |error: rusqlite::Error| SearchError::Store(error.into());
    let mut found = Vec::new();
    {
        // Materialized so SQLite cannot flatten bm25() into the windows.
        let mut statement = connection
            .prepare(
                "with matches as materialized (
                    select e.rowid, e.session, bm25(evidence_fts) as score
                    from evidence_fts join evidence e on e.rowid=evidence_fts.rowid
                    where evidence_fts match ?1
                 ), ranked as (
                    select rowid, session, count(*) over (partition by session) as matching,
                           min(score) over (partition by session) as best,
                           row_number() over (partition by session order by score, rowid) as n
                    from matches
                 ) select f.path, r.best, r.matching,
                          max(case when r.n=1 then r.rowid end),
                          max(case when r.n=2 then r.rowid end)
                   from ranked r join files f on f.key=r.session
                   where r.n<=2 group by r.session",
            )
            .map_err(store)?;
        let mut rows = statement.query([&query]).map_err(store)?;
        while let Some(row) = rows.next().map_err(store)? {
            let path: String = row.get(0).map_err(store)?;
            let Some(summary) = listed.remove(&path) else {
                continue;
            };
            found.push(Found {
                summary,
                best: row.get(1).map_err(store)?,
                matching: row.get(2).map_err(store)?,
                rowids: [row.get(3).map_err(store)?, row.get(4).map_err(store)?],
            });
        }
    }
    found.sort_by(|left, right| left.place().cmp_order(&right.place()));
    let total = found.len() as u64;
    let mut page: Vec<Found> = found
        .into_iter()
        .filter(|found| after.as_ref().is_none_or(|after| after.precedes(found)))
        .take(limit.get() + 1)
        .collect();
    let next_cursor = (page.len() > limit.get()).then(|| {
        page.truncate(limit.get());
        page.last().expect("a full page").place().encode()
    });
    // One pass over the matches for the whole page: FTS5 serves a rowid
    // constraint by running the match again per rowid, which for a short
    // prefix costs ~40 ms each, so `+` keeps the rowids a filter instead.
    let rowids: Vec<i64> = page
        .iter()
        .flat_map(|found| found.rowids.into_iter().flatten())
        .collect();
    let mut excerpts: HashMap<i64, SearchExcerpt> = connection
        .prepare(
            "select e.role, e.text, highlight(evidence_fts,0,char(30),char(31)),
                    e.at_kind, e.at_id, e.saved_at, e.rowid
             from evidence_fts join evidence e on e.rowid=evidence_fts.rowid
             where evidence_fts match ?1
               and +evidence_fts.rowid in (select value from json_each(?2))",
        )
        .map_err(store)?
        .query_map(
            params![query, serde_json::to_string(&rowids).expect("ids encode")],
            |row| Ok((row.get(6)?, excerpt(row)?)),
        )
        .map_err(store)?
        .collect::<rusqlite::Result<_>>()
        .map_err(store)?;
    let mut hits = Vec::with_capacity(page.len());
    for found in page {
        let mut shown: Vec<SearchExcerpt> = Vec::with_capacity(2);
        for excerpt in found
            .rowids
            .into_iter()
            .flatten()
            .filter_map(|rowid| excerpts.remove(&rowid))
        {
            if !shown
                .iter()
                .any(|previous| previous.role == excerpt.role && previous.text == excerpt.text)
            {
                shown.push(excerpt);
            }
        }
        let mut summary = found.summary;
        // Like the workspace, only the added directories still there, as a
        // listing shows them.
        summary.added_dirs.retain(|dir| dir.is_dir());
        hits.push(SearchHit {
            summary,
            matching_messages: found.matching,
            excerpts: shown,
        });
    }
    Ok(SearchPage {
        hits,
        next_cursor,
        total,
    })
}

/// Refuses a query [`search`] would: one without a word, or longer than
/// [`MAX_QUERY_CHARS`]. Callers check before refreshing the index.
pub(crate) fn check_query(query: &str) -> Result<(), SearchError> {
    if query.trim().is_empty() {
        return Err(SearchError::BlankQuery);
    }
    let chars = query.chars().count();
    if chars > MAX_QUERY_CHARS {
        return Err(SearchError::LongQuery(chars));
    }
    Ok(())
}

/// A matching session before its page is cut.
struct Found {
    summary: SessionSummary,
    /// Its best message's bm25 score: lower is more relevant.
    best: f64,
    matching: u64,
    /// Its best two matching messages, best first.
    rowids: [Option<i64>; 2],
}

impl Found {
    fn place(&self) -> Cursor<'_> {
        Cursor {
            updated_at: self.summary.updated_at,
            best: self.best,
            id: std::borrow::Cow::Borrowed(&self.summary.id),
            cwd: self.summary.cwd.to_string_lossy(),
        }
    }
}

/// A session's place in search order: newest `updated_at` first, then the
/// more relevant, then id and workspace, as the catalog orders. A cursor is
/// the place of the last session a page returned, so a walk repeats none
/// and skips none deleted between pages. Relevance moves as the index
/// changes, so a session saved in the same second as the cursor's may move
/// across it.
#[derive(Debug)]
struct Cursor<'a> {
    updated_at: u64,
    best: f64,
    id: std::borrow::Cow<'a, str>,
    cwd: std::borrow::Cow<'a, str>,
}

impl Cursor<'_> {
    fn encode(&self) -> String {
        serde_json::json!([self.updated_at, self.best, self.id, self.cwd]).to_string()
    }

    /// The cursor a page's `next_cursor` encoded, or `None` for a string no
    /// search issued.
    fn parse(cursor: &str) -> Option<Cursor<'static>> {
        serde_json::from_str::<(u64, f64, String, String)>(cursor)
            .ok()
            .map(|(updated_at, best, id, cwd)| Cursor {
                updated_at,
                best,
                id: id.into(),
                cwd: cwd.into(),
            })
    }

    fn cmp_order(&self, other: &Cursor<'_>) -> Ordering {
        other
            .updated_at
            .cmp(&self.updated_at)
            .then_with(|| self.best.total_cmp(&other.best))
            .then_with(|| self.id.cmp(&other.id))
            .then_with(|| self.cwd.cmp(&other.cwd))
    }

    fn precedes(&self, found: &Found) -> bool {
        self.cmp_order(&found.place()) == Ordering::Less
    }
}

/// A matching message as a hit shows it, from the excerpt query's row.
fn excerpt(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchExcerpt> {
    let role = match row.get_ref(0)?.as_str()? {
        "user" => ExcerptRole::User,
        "assistant" | "assistant_aborted" => ExcerptRole::Agent,
        "tool_result" => ExcerptRole::ToolResult,
        "tool_error" => ExcerptRole::ToolError,
        _ => ExcerptRole::Unknown,
    };
    let text: String = row.get(1)?;
    let highlighted: String = row.get(2)?;
    let shown = window(&text, &highlighted);
    let at = match (
        row.get_ref(3)?.as_str_or_null()?,
        row.get::<_, Option<String>>(4)?,
    ) {
        (Some("message"), Some(id)) => Some(SearchLocation::Message {
            id: MessageId::new(id),
        }),
        (Some("tool_call"), Some(id)) => Some(SearchLocation::ToolCall {
            id: ToolCallId::new(id),
        }),
        _ => None,
    };
    Ok(SearchExcerpt::new(
        role,
        shown.text,
        shown
            .highlights
            .into_iter()
            // A window is a few hundred bytes.
            .map(|range| range.start as u32..range.end as u32)
            .collect(),
        at,
        row.get(5)?,
    ))
}
