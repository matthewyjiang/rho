//! The session index's SQLite schema: creating it, migrating older ones in
//! place, and refusing ones this Rho cannot read.

use std::collections::HashSet;

use rusqlite::{Connection, Transaction, TransactionBehavior};

pub(super) const INDEX_SCHEMA_VERSION: u32 = 3;

pub(super) fn migrate_index(connection: &mut Connection) -> anyhow::Result<()> {
    migrate_index_with_hook(connection, |_| Ok(()))
}

pub(super) fn migrate_index_with_hook(
    connection: &mut Connection,
    before_commit: impl FnOnce(&Transaction<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    // IMMEDIATE waits on the busy handler for other writers (a deferred
    // transaction upgrading to a write fails at once with BUSY_SNAPSHOT in
    // WAL), and reading the version under it serializes the migration decision.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: u32 = transaction.query_row("pragma user_version", [], |row| row.get(0))?;
    if version > INDEX_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported session index schema {version} (maximum supported: {INDEX_SCHEMA_VERSION})"
        );
    }
    if version > 0 {
        validate_legacy_index_columns(&transaction)?;
    }
    if version == 0 {
        transaction.execute_batch(
            "create table if not exists sessions (
            workspace_key text not null,
            cwd text not null,
            id text not null,
            path text not null,
            created_at integer not null,
            updated_at integer not null,
            message_count integer not null default 0,
            title text,
            first_user_message text,
            last_user_message text,
            added_dirs text,
            file_size integer,
            file_mtime integer,
            node_count integer not null default 0,
            branch_count integer not null default 0,
            active_leaf_id text,
            effective_format_version integer not null default 1,
            primary key (workspace_key, id)
        );",
        )?;
        ensure_column(&transaction, "title text")?;
        ensure_column(&transaction, "first_user_message text")?;
    }
    // Added without a version bump: older Rho validates only the columns it
    // reads, so it keeps using an index that has this one. Rows without it,
    // or with a stale one, re-summarize; see `StampedDirs`.
    ensure_column(&transaction, "added_dirs text")?;
    ensure_column(&transaction, "node_count integer not null default 0")?;
    ensure_column(&transaction, "branch_count integer not null default 0")?;
    ensure_column(&transaction, "active_leaf_id text")?;
    ensure_column(
        &transaction,
        "effective_format_version integer not null default 1",
    )?;
    // Versioned, unlike the columns above: an older Rho that indexed a run
    // or child session would leave its origin unset, listing it as a
    // person's, and its parent unset, keeping it when its parent is deleted.
    // A parent is its workspace and id, since ids repeat across workspaces.
    ensure_column(&transaction, "origin text")?;
    ensure_column(&transaction, "parent_workspace_key text")?;
    ensure_column(&transaction, "parent_id text")?;
    if version < 3 {
        // Force the next workspace sync to re-summarize existing rows,
        // backfilling tree facts (version 2) and header origins and parents
        // (version 3).
        transaction.execute("update sessions set file_size = null", [])?;
    }
    validate_index_columns(&transaction)?;
    transaction.execute_batch(
        "create index if not exists sessions_workspace_updated_idx
            on sessions(workspace_key, updated_at desc);
         create index if not exists sessions_workspace_id_idx
            on sessions(workspace_key, id);
         create index if not exists sessions_parent_idx
            on sessions(parent_workspace_key, parent_id);",
    )?;
    transaction.pragma_update(None, "user_version", INDEX_SCHEMA_VERSION)?;
    before_commit(&transaction)?;
    crate::session::search_journal::install(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn validate_legacy_index_columns(connection: &Connection) -> anyhow::Result<()> {
    const REQUIRED_COLUMNS: &[&str] = &[
        "workspace_key",
        "cwd",
        "id",
        "path",
        "created_at",
        "updated_at",
        "message_count",
        "last_user_message",
        "file_size",
        "file_mtime",
    ];
    validate_columns(connection, REQUIRED_COLUMNS)
}

pub(super) fn validate_index_columns(connection: &Connection) -> anyhow::Result<()> {
    const REQUIRED_COLUMNS: &[&str] = &[
        "workspace_key",
        "cwd",
        "id",
        "path",
        "created_at",
        "updated_at",
        "message_count",
        "title",
        "first_user_message",
        "last_user_message",
        "added_dirs",
        "file_size",
        "file_mtime",
        "node_count",
        "branch_count",
        "active_leaf_id",
        "effective_format_version",
        "origin",
        "parent_workspace_key",
        "parent_id",
    ];
    validate_columns(connection, REQUIRED_COLUMNS)
}

fn validate_columns(connection: &Connection, required: &[&str]) -> anyhow::Result<()> {
    let columns = session_table_columns(connection)?;
    let missing = required
        .iter()
        .filter(|column| !columns.contains(**column))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        anyhow::bail!(
            "malformed session index schema: missing column(s): {}",
            missing.join(", ")
        );
    }
    Ok(())
}

fn ensure_column(connection: &Connection, column_definition: &str) -> anyhow::Result<()> {
    let column_name = column_definition
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("column definition must include a name"))?;
    if !session_table_columns(connection)?.contains(column_name) {
        connection.execute(
            &format!("alter table sessions add column {column_definition}"),
            [],
        )?;
    }
    Ok(())
}

fn session_table_columns(connection: &Connection) -> anyhow::Result<HashSet<String>> {
    let mut statement = connection.prepare("pragma table_info(sessions)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok(columns)
}
