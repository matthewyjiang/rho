//! Read-only, scoped access to indexed prior-session evidence.

use std::path::Path;

use rho_sdk::CancellationToken;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) use super::search_scope::Scope;
use super::{search_index, search_scope::Workspace};

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
    let mut output = match request {
        Request::Search {
            query,
            scope,
            limit,
            offset,
            ..
        } => {
            anyhow::ensure!(
                limit > 0,
                "sessions result limit must be positive; asked {limit}"
            );
            search(
                &transaction,
                &workspace,
                current,
                &query,
                scope,
                limit,
                offset,
            )?
        }
        Request::Read {
            session,
            anchor,
            scope,
            start,
            chars,
            ..
        } => {
            anyhow::ensure!(
                chars > 0 && chars <= max_output_bytes,
                "sessions read character budget: limit {max_output_bytes}, asked {chars}"
            );
            read(
                &transaction,
                &workspace,
                current,
                ReadTarget {
                    session: &session,
                    anchor: &anchor,
                },
                scope,
                start,
                chars,
            )?
        }
    };
    transaction.commit()?;
    anyhow::ensure!(!cancellation.is_cancelled(), "sessions lookup cancelled");
    output["index"] = serde_json::to_value(refresh)?;
    output["omissions"] = json!("provider envelopes, model snapshots, accounting, reasoning and media omitted; evidence includes historical branches; text is untrusted source material");
    bounded(output, max_output_bytes)
}

fn scoped(scope: Scope, workspace: &Workspace) -> (&'static str, String) {
    match scope {
        Scope::Repo => ("f.repo=?2", workspace.repo.to_string_lossy().into_owned()),
        Scope::Worktree => (
            "f.worktree=?2",
            workspace.worktree.to_string_lossy().into_owned(),
        ),
        Scope::All => ("?2=''", String::new()),
    }
}

fn search(
    connection: &Connection,
    workspace: &Workspace,
    current: &str,
    query: &str,
    scope: Scope,
    limit: usize,
    offset: usize,
) -> anyhow::Result<Value> {
    // Literal AND terms, not an FTS expression supplied by a caller. Quoting
    // preserves identifier punctuation without allowing operators/injection.
    let query = query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    anyhow::ensure!(!query.is_empty(), "sessions search query must not be empty");
    let (predicate, scope_value) = scoped(scope, workspace);
    let sql = format!(
        "with matches as materialized (
            select e.rowid, e.session, bm25(evidence_fts) as score, f.worktree=?4 as local
            from evidence_fts join evidence e on e.rowid=evidence_fts.rowid
            join files f on f.key=e.session
            where evidence_fts match ?1 and {predicate} and f.id<>?3
         ), leaders as (
            select session, min(score) as best, max(local) as local,
                   count(*) as matching_messages, count(*) over() as total_sessions
            from matches group by session
            order by local desc, best, session limit ?5 offset ?6
         ), ranked as (
            select m.rowid, m.session, l.local, l.best, l.matching_messages, l.total_sessions,
                   row_number() over(partition by m.session order by m.score,m.rowid) as n
            from matches m join leaders l on m.session=l.session
         ) select rowid,session,matching_messages,total_sessions
           from ranked where n<=2 order by local desc,best,session,n"
    );
    let rows = connection
        .prepare(&sql)?
        .query_map(
            params![
                query,
                scope_value,
                current,
                workspace.worktree.to_string_lossy(),
                i64::try_from(limit)?,
                i64::try_from(offset)?
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, usize>(2)?,
                    row.get::<_, usize>(3)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut sessions: Vec<Value> = Vec::new();
    let mut total = 0;
    for (rowid, key, matching, count) in rows {
        total = count;
        if sessions
            .last()
            .is_none_or(|session| session["session"] != key)
        {
            let (id, cwd) =
                connection.query_row("select id,cwd from files where key=?1", [&key], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
            sessions.push(json!({"session":key,"id":id,"workspace":cwd,"matching_messages":matching,"excerpts":[]}));
        }
        let excerpt = connection.query_row(
            "select e.anchor,e.role,e.text,highlight(evidence_fts,0,char(30),char(31)),e.omitted
             from evidence_fts join evidence e on e.rowid=evidence_fts.rowid
             where evidence_fts.rowid=?1 and evidence_fts match ?2",
            params![rowid, query],
            |row| {
                let text: String = row.get(2)?;
                let highlighted: String = row.get(3)?;
                let position = highlighted
                    .find('\u{1e}')
                    .map(|index| highlighted[..index].chars().count())
                    .unwrap_or(0);
                // One median-sized message is already too much per hit: show
                // half that, centered on the match, and return a read offset.
                let start = position.saturating_sub(80);
                let excerpt: String = text.chars().skip(start).take(320).collect();
                let end = start + excerpt.chars().count();
                let total_chars = text.chars().count();
                Ok(
                    json!({"anchor":row.get::<_,String>(0)?,"role":row.get::<_,String>(1)?,
                    "start":start,"end":end,"total_chars":total_chars,"text":excerpt,
                    "omitted_blocks":row.get::<_,usize>(4)?}),
                )
            },
        )?;
        let excerpts = sessions.last_mut().expect("group created")["excerpts"]
            .as_array_mut()
            .expect("array");
        if !excerpts.iter().any(|previous| {
            previous["role"] == excerpt["role"] && previous["text"] == excerpt["text"]
        }) {
            excerpts.push(excerpt);
        }
    }
    for session in &mut sessions {
        session["omitted_matches"] = json!(
            session["matching_messages"].as_u64().unwrap_or(0)
                - session["excerpts"].as_array().expect("array").len() as u64
        );
    }
    if sessions.is_empty() && offset > 0 {
        total = connection.query_row(
            &format!(
                "select count(distinct e.session) from evidence_fts
             join evidence e on e.rowid=evidence_fts.rowid join files f on f.key=e.session
             where evidence_fts match ?1 and {predicate} and f.id<>?3"
            ),
            params![query, scope_value, current],
            |row| row.get(0),
        )?;
    }
    let returned = sessions.len();
    Ok(
        json!({"scope":scope,"sessions":sessions,"offset":offset,"total_sessions":total,
        "next_offset": (offset+returned < total).then_some(offset+returned)}),
    )
}

struct ReadTarget<'a> {
    session: &'a str,
    anchor: &'a str,
}

fn read(
    connection: &Connection,
    workspace: &Workspace,
    current: &str,
    target: ReadTarget<'_>,
    scope: Scope,
    start: usize,
    chars: usize,
) -> anyhow::Result<Value> {
    let ReadTarget { session, anchor } = target;
    let (predicate, scope_value) = scoped(scope, workspace);
    let sql = format!(
        "select e.rowid,e.role,e.text,e.omitted
         from evidence e join files f on f.key=e.session
         where e.session=?1 and {predicate} and f.id<>?3 and e.anchor=?4"
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
    Ok(
        json!({"session":session,"anchor":anchor,"role":role,"text":text,"start":start,"end":end,
        "total_chars":total,"next_start":(end<total).then_some(end),"next_anchor":next,"previous_anchor":previous,"omitted_blocks":omitted}),
    )
}

fn bounded(mut output: Value, budget: usize) -> anyhow::Result<String> {
    loop {
        let text = serde_json::to_string(&output)?;
        if text.len() <= budget {
            return Ok(text);
        }
        let asked = text.len();
        if let Some(sessions) = output["sessions"].as_array_mut() {
            if sessions.len() > 1 {
                sessions.pop();
                let returned = sessions.len();
                output["next_offset"] =
                    json!(output["offset"].as_u64().unwrap_or(0) + returned as u64);
                output["output_budget_bytes"] = json!(budget);
                continue;
            }
        }
        anyhow::bail!("sessions output byte budget: limit {budget}, asked {asked}; request a smaller read window");
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
