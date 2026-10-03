//! Read-only, scoped access to indexed prior-session evidence.

use std::path::Path;

use rho_sdk::CancellationToken;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

pub(crate) use super::search_scope::Scope;
use super::{search_index, workspace_scope::Workspace};

/// Model-facing name of the tool serving this module. Search excludes that
/// tool's own calls and results from evidence, so the name lives here.
pub(crate) const TOOL_NAME: &str = "sessions";

#[path = "search_response.rs"]
mod response;
pub(super) use response::ensure_budget;
use response::{Context, Excerpt, Group, Matches, Page, ReadResponse, Sessions};

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    Search {
        query: String,
        #[serde(default)]
        scope: Scope,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(default)]
        refresh: bool,
    },
    Read {
        session: String,
        anchor: String,
        #[serde(default)]
        scope: Scope,
        #[serde(default)]
        start: usize,
        #[serde(default = "default_chars")]
        chars: usize,
        #[serde(default)]
        refresh: bool,
    },
}

impl Request {
    pub(crate) fn validate(&self, budget: usize) -> anyhow::Result<()> {
        match self {
            Self::Search {
                query,
                limit,
                offset,
                ..
            } => {
                anyhow::ensure!(
                    !query.trim().is_empty(),
                    "sessions search query must not be empty"
                );
                anyhow::ensure!(
                    *limit > 0,
                    "sessions result limit must be positive; asked {limit}"
                );
                i64::try_from(*limit)?;
                i64::try_from(*offset)?;
            }
            Self::Read { chars, start, .. } => {
                anyhow::ensure!(
                    *chars > 0 && *chars <= budget,
                    "sessions read character budget: limit {budget}, asked {chars}"
                );
                i64::try_from(*start)?;
            }
        }
        Ok(())
    }
}

// Five groups with two excerpts fit a small discovery turn. The corpus median
// message is 575 chars and p95 is 7,348; focused reads page rather than dump it.
fn default_limit() -> usize {
    5
}
fn default_chars() -> usize {
    4096
}

pub(crate) fn execute(
    root: &Path,
    cwd: &Path,
    current: &str,
    request: Request,
    max_output_bytes: usize,
    cancellation: &CancellationToken,
) -> anyhow::Result<String> {
    anyhow::ensure!(!cancellation.is_cancelled(), "sessions lookup cancelled");
    anyhow::ensure!(
        !current.is_empty(),
        "sessions requires a bound current session"
    );
    request.validate(max_output_bytes)?;
    let reconcile = match &request {
        Request::Search { refresh, .. } | Request::Read { refresh, .. } => *refresh,
    };
    let workspace = Workspace::resolve(cwd);
    let mut connection = search_index::open(root)?;
    let refresh = search_index::refresh(&mut connection, root, reconcile, cancellation)?;
    let transaction = connection.transaction()?;
    let context = Context::new(refresh);
    let output = match request {
        Request::Search {
            query,
            scope,
            limit,
            offset,
            ..
        } => {
            let target = SearchTarget {
                query: &query,
                scope,
                limit,
                offset,
            };
            match scope {
                Scope::Current => search_current(
                    &transaction,
                    current,
                    target,
                    context,
                    max_output_bytes,
                    cancellation,
                )?,
                Scope::Repo | Scope::Worktree | Scope::All => search(
                    &transaction,
                    &workspace,
                    current,
                    target,
                    context,
                    max_output_bytes,
                    cancellation,
                )?,
            }
        }
        Request::Read {
            session,
            anchor,
            scope,
            start,
            chars,
            ..
        } => read(
            &transaction,
            &workspace,
            current,
            ReadTarget {
                session: &session,
                anchor: &anchor,
                start,
                chars,
            },
            scope,
            context,
        )?
        .finish(max_output_bytes)?,
    };
    transaction.commit()?;
    anyhow::ensure!(!cancellation.is_cancelled(), "sessions lookup cancelled");
    Ok(output)
}

/// SQL predicate over `files f` binding ?2 to the scope value and ?3 to the
/// current session id. Only `Current` admits the current session; its search
/// filters by session handle instead (see `search_current`), reads use this.
fn scoped(scope: Scope, workspace: &Workspace) -> (&'static str, String) {
    match scope {
        Scope::Repo => (
            "f.repo=?2 and f.id<>?3",
            workspace.repo.to_string_lossy().into_owned(),
        ),
        Scope::Worktree => (
            "f.worktree=?2 and f.id<>?3",
            workspace.worktree.to_string_lossy().into_owned(),
        ),
        Scope::All => ("?2='' and f.id<>?3", String::new()),
        Scope::Current => ("?2='' and f.id=?3", String::new()),
    }
}

/// Literal AND terms, not an FTS expression supplied by a caller. Quoting
/// preserves identifier punctuation without allowing operators/injection.
fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

const EXCERPT_SQL: &str =
    "select e.anchor,e.role,e.text,highlight(evidence_fts,0,char(30),char(31)),e.omitted
     from evidence_fts join evidence e on e.rowid=evidence_fts.rowid
     where evidence_fts.rowid=?1 and evidence_fts match ?2";

/// One median-sized message is already too much per hit: show half that,
/// centered on the first match, and return a read offset. Reads `EXCERPT_SQL` rows.
fn excerpt(row: &rusqlite::Row<'_>) -> rusqlite::Result<Excerpt> {
    let text: String = row.get(2)?;
    let highlighted: String = row.get(3)?;
    let position = highlighted
        .find('\u{1e}')
        .map(|index| highlighted[..index].chars().count())
        .unwrap_or(0);
    let start = position.saturating_sub(80);
    let excerpt: String = text.chars().skip(start).take(320).collect();
    let end = start + excerpt.chars().count();
    Ok(Excerpt {
        anchor: row.get(0)?,
        role: row.get(1)?,
        start,
        end,
        total_chars: text.chars().count(),
        text: excerpt,
        omitted_blocks: row.get(4)?,
    })
}

struct SearchTarget<'a> {
    query: &'a str,
    scope: Scope,
    limit: usize,
    offset: usize,
}

fn search(
    connection: &Connection,
    workspace: &Workspace,
    current: &str,
    target: SearchTarget<'_>,
    context: Context,
    budget: usize,
    cancellation: &CancellationToken,
) -> anyhow::Result<String> {
    let SearchTarget {
        query,
        scope,
        limit,
        offset,
    } = target;
    let query = fts_query(query);
    let (predicate, scope_value) = scoped(scope, workspace);
    let sql = format!(
        "with matches as materialized (
            select e.rowid, e.session, bm25(evidence_fts) as score, f.worktree=?4 as local
            from evidence_fts join evidence e on e.rowid=evidence_fts.rowid
            join files f on f.key=e.session
            where evidence_fts match ?1 and {predicate}
         ), leaders as (
            select session, min(score) as best, max(local) as local,
                   count(*) as matching_messages, count(*) over() as total_sessions
            from matches group by session
            order by local desc, best, session limit ?5 offset ?6
         ), ranked as (
            select m.rowid, m.session, l.local, l.best, l.matching_messages, l.total_sessions,
                   row_number() over(partition by m.session order by m.score,m.rowid) as n
            from matches m join leaders l on m.session=l.session
         ) select r.session,f.id,f.cwd,r.matching_messages,r.total_sessions,
                  max(case when n=1 then r.rowid end),max(case when n=2 then r.rowid end)
           from ranked r join files f on f.key=r.session where n<=2
           group by r.session order by r.local desc,r.best,r.session"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(params![
        query,
        scope_value,
        current,
        workspace.worktree.to_string_lossy(),
        i64::try_from(limit)?,
        i64::try_from(offset)?
    ])?;
    let mut row = rows.next()?;
    let total = if let Some(row) = row {
        row.get(4)?
    } else if offset > 0 {
        connection.query_row(
            &format!(
                "select count(distinct e.session) from evidence_fts
             join evidence e on e.rowid=evidence_fts.rowid join files f on f.key=e.session
             where evidence_fts match ?1 and {predicate}"
            ),
            params![query, scope_value, current],
            |row| row.get(0),
        )?
    } else {
        0
    };
    let mut page = Page::new(Sessions, context, scope, offset, total, limit, budget);
    let mut excerpts = connection.prepare(EXCERPT_SQL)?;
    while let Some(current_row) = row {
        anyhow::ensure!(!cancellation.is_cancelled(), "sessions lookup cancelled");
        let mut group = Group {
            session: current_row.get(0)?,
            id: current_row.get(1)?,
            workspace: current_row.get(2)?,
            matching_messages: current_row.get(3)?,
            excerpts: Vec::new(),
            omitted_matches: 0,
        };
        for rowid in [current_row.get::<_, Option<i64>>(5)?, current_row.get(6)?]
            .into_iter()
            .flatten()
        {
            let excerpt = excerpts.query_row(params![rowid, query], excerpt)?;
            if !group
                .excerpts
                .iter()
                .any(|previous| previous.role == excerpt.role && previous.text == excerpt.text)
            {
                group.excerpts.push(excerpt);
            }
        }
        group.omitted_matches = group.matching_messages - group.excerpts.len();
        if !page.push(group)? {
            break;
        }
        row = rows.next()?;
    }
    page.finish()
}

/// Pages the current session's matching messages rather than session groups:
/// there is only one session, so a two-excerpt group would hide the rest.
/// Identical role/text evidence (repeated across records) is listed once.
fn search_current(
    connection: &Connection,
    current: &str,
    target: SearchTarget<'_>,
    context: Context,
    budget: usize,
    cancellation: &CancellationToken,
) -> anyhow::Result<String> {
    let SearchTarget {
        query,
        scope,
        limit,
        offset,
    } = target;
    let query = fts_query(query);
    // Unsaved or not yet indexed sessions have no handle and no matches.
    let session: Option<String> = connection
        .query_row(
            "select key from files where id=?1 order by key limit 1",
            [current],
            |row| row.get(0),
        )
        .optional()?;
    // Materialized so SQLite cannot flatten bm25() into the aggregate.
    let distinct_matches = "with matches as materialized (
            select e.rowid, e.role, e.text, bm25(evidence_fts) as score
            from evidence_fts join evidence e on e.rowid=evidence_fts.rowid
            where evidence_fts match ?1 and e.session=?2
        ), distinct_matches as (
            select min(rowid) as rowid, min(score) as score from matches group by role, text
        )";
    let mut statement = connection.prepare(&format!(
        "{distinct_matches} select rowid, count(*) over() from distinct_matches
         order by score, rowid limit ?3 offset ?4"
    ))?;
    let mut rows = statement.query(params![
        query,
        session,
        i64::try_from(limit)?,
        i64::try_from(offset)?
    ])?;
    let mut row = rows.next()?;
    let total = match row {
        Some(row) => row.get(1)?,
        None if offset > 0 => connection.query_row(
            &format!("{distinct_matches} select count(*) from distinct_matches"),
            params![query, session],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let mut page = Page::new(
        Matches { session },
        context,
        scope,
        offset,
        total,
        limit,
        budget,
    );
    let mut excerpts = connection.prepare(EXCERPT_SQL)?;
    while let Some(match_row) = row {
        anyhow::ensure!(!cancellation.is_cancelled(), "sessions lookup cancelled");
        let rowid: i64 = match_row.get(0)?;
        if !page.push(excerpts.query_row(params![rowid, query], excerpt)?)? {
            break;
        }
        row = rows.next()?;
    }
    page.finish()
}

struct ReadTarget<'a> {
    session: &'a str,
    anchor: &'a str,
    start: usize,
    chars: usize,
}

fn read(
    connection: &Connection,
    workspace: &Workspace,
    current: &str,
    target: ReadTarget<'_>,
    scope: Scope,
    context: Context,
) -> anyhow::Result<ReadResponse> {
    let ReadTarget {
        session,
        anchor,
        start,
        chars,
    } = target;
    let (predicate, scope_value) = scoped(scope, workspace);
    let sql = format!(
        "select e.rowid,e.role,e.text,e.omitted
         from evidence e join files f on f.key=e.session
         where e.session=?1 and {predicate} and e.anchor=?4"
    );
    let row = connection.query_row(&sql, params![session,scope_value,current,anchor], |row| {
        Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,usize>(3)?))
    }).optional()?.ok_or_else(|| anyhow::anyhow!("session anchor not found in requested scope; search again if the transcript changed"))?;
    let (rowid, role, text, omitted) = row;
    // SQLite text length/substr stop at NUL. Rust windows preserve raw tool
    // errors containing NUL and use the same Unicode offsets as search.
    let total = text.chars().count();
    let text: String = text.chars().skip(start).take(chars).collect();
    anyhow::ensure!(
        start <= total,
        "sessions read offset: limit {total} characters, asked {start}"
    );
    let next: Option<String> = connection
        .query_row(
            "select anchor from evidence where session=?1 and rowid>?2 order by rowid limit 1",
            params![session, rowid],
            |row| row.get(0),
        )
        .optional()?;
    let previous: Option<String> = connection
        .query_row(
            "select anchor from evidence where session=?1 and rowid<?2 order by rowid desc limit 1",
            params![session, rowid],
            |row| row.get(0),
        )
        .optional()?;
    let end = start + text.chars().count();
    Ok(ReadResponse {
        context,
        session: session.to_owned(),
        anchor: anchor.to_owned(),
        role,
        text,
        start,
        end,
        total_chars: total,
        next_start: (end < total).then_some(end),
        next_anchor: next,
        previous_anchor: previous,
        omitted_blocks: omitted,
    })
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
