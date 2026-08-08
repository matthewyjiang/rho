//! Cross-project session listing against the global session index.
//!
//! Reconciliation mechanics live in the index module; this module owns the
//! canonical global summary queries.

use std::path::Path;

use rusqlite::params;

use super::{open_index, reconcile_all_workspaces, summary_from_row, SessionSummary};

/// Lists sessions across every workspace under `session_root`.
///
/// Reconciles the global index against on-disk units in one pass, then reads
/// summaries from SQLite. This avoids the per-workspace sync loop that made
/// cross-project listing scale with workspace count instead of change count.
pub(in crate::session) fn list_all_sessions(
    session_root: &Path,
) -> anyhow::Result<Vec<SessionSummary>> {
    reconcile_all_workspaces(session_root)?;
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let mut statement = connection.prepare(
        "select id, path, cwd, created_at, updated_at, message_count,
                title, first_user_message, last_user_message
         from (
             select id, path, cwd, created_at, updated_at, message_count,
                    title, first_user_message, last_user_message,
                    row_number() over (
                        partition by path
                        order by updated_at desc, cwd asc
                    ) as rn
             from sessions
         )
         where rn = 1
         order by updated_at desc, created_at desc, id asc",
    )?;
    let rows = statement.query_map([], summary_from_row)?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|summary| summary.path.exists())
        .collect())
}

/// Returns index summaries whose id starts with `id_prefix` (any workspace).
///
/// Does not scan the filesystem. Callers that need brand-new unindexed units
/// should use [`list_all_sessions`] or a workspace list.
pub(in crate::session) fn summaries_matching_id_prefix(
    session_root: &Path,
    id_prefix: &str,
) -> anyhow::Result<Vec<SessionSummary>> {
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let mut statement = connection.prepare(
        "select id, path, cwd, created_at, updated_at, message_count,
                title, first_user_message, last_user_message
         from (
             select id, path, cwd, created_at, updated_at, message_count,
                    title, first_user_message, last_user_message,
                    row_number() over (
                        partition by path
                        order by updated_at desc, cwd asc
                    ) as rn
             from sessions
             where substr(id, 1, length(?1)) = ?1
         )
         where rn = 1
         order by updated_at desc, created_at desc, id asc",
    )?;
    let rows = statement.query_map(params![id_prefix], summary_from_row)?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|summary| summary.path.exists())
        .collect())
}
