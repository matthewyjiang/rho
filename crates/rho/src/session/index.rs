use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

const INDEX_SCHEMA_VERSION: u32 = 2;

static INDEX_CONNECTIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>> =
    OnceLock::new();

#[cfg(test)]
pub(super) fn clear_index_connection_cache_for_test() {
    if let Some(connections) = INDEX_CONNECTIONS.get() {
        connections
            .lock()
            .expect("session index cache poisoned")
            .clear();
    }
}

use super::persistence::{
    clamp_u64_to_i64, read_session_cwd, session_dir_in_root, session_file_stats,
    session_id_from_path, set_private_dir_permissions, summarize_session_file, workspace_key,
    SessionUnit,
};
use super::{Session, SessionIndexRecord, SessionSummary};

#[path = "index_list.rs"]
mod list;
pub(super) use list::{list_all_sessions, summaries_matching_id_prefix};

pub(super) fn list_workspace_sessions(
    session_root: &Path,
    cwd: &Path,
) -> anyhow::Result<Vec<SessionSummary>> {
    sync_workspace(session_root, cwd)?;
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let workspace_key = workspace_key(cwd);
    let mut statement = connection.prepare(
        "select id, path, cwd, created_at, updated_at, message_count,
                title, first_user_message, last_user_message
         from sessions
         where workspace_key = ?1
         order by updated_at desc, created_at desc, id asc",
    )?;
    let rows = statement.query_map(params![workspace_key], summary_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn matching_session_paths(
    session_root: &Path,
    cwd: &Path,
    id_prefix: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    query_existing_paths(
        &connection,
        "select path
         from sessions
         where workspace_key = ?1 and substr(id, 1, length(?2)) = ?2
         order by id asc",
        params![workspace_key(cwd), id_prefix],
    )
}

/// Resolves a session by id prefix across every workspace, returning each
/// match's file path and the workspace it belongs to, so a session can be
/// recovered by id from any directory and resumed under its own workspace.
///
/// A session file can accrue multiple index rows when it is recorded under more
/// than one `(workspace_key, id)`. Collapse those to one row per `path` (newest
/// `updated_at`, then stable `cwd`) so an unambiguous id still resolves once.
pub(super) fn matching_sessions_any_workspace(
    session_root: &Path,
    id_prefix: &str,
) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    // One row per path: `distinct path, cwd` still returns multiple rows when
    // the same file was indexed under different workspaces with different cwds.
    let mut statement = connection.prepare(
        "select path, cwd
         from (
             select path,
                    cwd,
                    row_number() over (
                        partition by path
                        order by updated_at desc, cwd asc
                    ) as rn
             from sessions
             where substr(id, 1, length(?1)) = ?1
         )
         where rn = 1
         order by path asc",
    )?;
    let rows = statement.query_map(params![id_prefix], |row| {
        let path: String = row.get(0)?;
        let cwd: String = row.get(1)?;
        Ok((PathBuf::from(path), PathBuf::from(cwd)))
    })?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(path, _)| path.exists())
        .collect())
}

/// Runs a `select path` query and returns the paths whose files still exist.
fn query_existing_paths(
    connection: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(params, |row| {
        let path: String = row.get(0)?;
        Ok(PathBuf::from(path))
    })?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| path.exists())
        .collect())
}

pub(super) fn sync_workspace(session_root: &Path, cwd: &Path) -> anyhow::Result<()> {
    reconcile_sessions(
        session_root,
        ReconcileScope::Workspace {
            workspace_key: workspace_key(cwd),
            dir: session_dir_in_root(session_root, cwd),
        },
    )
}

pub(super) fn reconcile_all_workspaces(session_root: &Path) -> anyhow::Result<()> {
    reconcile_sessions(session_root, ReconcileScope::All)
}

type IndexKey = (String, String);

enum ReconcileScope {
    Workspace { workspace_key: String, dir: PathBuf },
    All,
}

struct ScannedSession {
    key: IndexKey,
    transcript: PathBuf,
    cwd: PathBuf,
}

fn reconcile_sessions(session_root: &Path, scope: ReconcileScope) -> anyhow::Result<()> {
    reconcile_sessions_with_hook(session_root, scope, || Ok(()))
}

fn reconcile_sessions_with_hook<F>(
    session_root: &Path,
    scope: ReconcileScope,
    before_transaction: F,
) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    let connection = open_index(session_root)?;
    let indexed_files = {
        let connection = connection
            .lock()
            .expect("session index connection poisoned");
        indexed_files_for_scope(&connection, &scope)?
    };

    let scanned = scan_sessions(session_root, &scope)?;
    let seen = scanned
        .iter()
        .map(|session| session.key.clone())
        .collect::<HashSet<_>>();
    let changed = scanned.into_iter().filter(|session| {
        let (file_size, file_mtime) = session_file_stats(&session.transcript);
        !indexed_files.get(&session.key).is_some_and(|indexed| {
            indexed.cwd == session.cwd.to_string_lossy()
                && indexed_record_belongs_to_workspace(session_root, &session.key, indexed)
                && indexed.is_current(&session.transcript, file_size, file_mtime)
        })
    });

    // Transcript parsing stays outside the index lock.
    let mut records = Vec::new();
    for session in changed {
        if let Ok(record) = summarize_session_file(&session.transcript, &session.cwd) {
            if record.summary.cwd == session.cwd {
                records.push((session.key, record));
            }
        }
    }

    let refreshed = records
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<HashSet<_>>();
    let mut stale_keys = stale_index_keys(&indexed_files, &seen, &refreshed);
    stale_keys.extend(
        indexed_files
            .iter()
            .filter(|(key, indexed)| {
                !refreshed.contains(*key)
                    && !indexed_record_belongs_to_workspace(session_root, key, indexed)
            })
            .map(|(key, _)| key.clone()),
    );
    stale_keys.sort();
    stale_keys.dedup();

    before_transaction()?;
    let mut connection = connection
        .lock()
        .expect("session index connection poisoned");
    // IMMEDIATE excludes writers from other Rho processes too. Keep transcript
    // parsing above this point, then revalidate the index and filesystem while
    // the write claim prevents a stale snapshot from deleting a concurrent sync.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current_indexed = indexed_files_for_scope(&transaction, &scope)?;
    records.retain(|(key, record)| {
        let path = &record.summary.path;
        let (file_size, file_mtime) = session_file_stats(path);
        record.file_size == file_size
            && record.file_mtime == file_mtime
            && transcript_matches_workspace(session_root, key, path, &record.summary.cwd)
            && !current_indexed.get(key).is_some_and(|indexed| {
                indexed.cwd == record.summary.cwd.to_string_lossy()
                    && indexed_record_belongs_to_workspace(session_root, key, indexed)
                    && indexed.is_current(path, file_size, file_mtime)
            })
    });
    let refreshed = records
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<HashSet<_>>();

    let current_seen = scan_sessions(session_root, &scope)?
        .into_iter()
        .map(|session| session.key)
        .collect::<HashSet<_>>();
    stale_keys.retain(|key| !current_seen.contains(key));
    stale_keys.extend(
        indexed_files
            .iter()
            .filter(|(key, indexed)| {
                !refreshed.contains(*key)
                    && (!current_seen.contains(*key)
                        || !indexed_record_belongs_to_workspace(session_root, key, indexed))
            })
            .map(|(key, _)| key.clone()),
    );
    stale_keys.sort();
    stale_keys.dedup();
    let stale_keys = stale_keys
        .into_iter()
        .filter(|key| current_indexed.get(key) == indexed_files.get(key))
        .collect::<Vec<_>>();

    apply_reconciliation_transaction(transaction, &records, &stale_keys)
}

fn scan_sessions(
    session_root: &Path,
    scope: &ReconcileScope,
) -> anyhow::Result<Vec<ScannedSession>> {
    let mut scanned = Vec::new();
    match scope {
        ReconcileScope::Workspace {
            workspace_key, dir, ..
        } => scan_workspace_dir(dir, workspace_key, &mut scanned)?,
        ReconcileScope::All => {
            if !session_root.is_dir() {
                return Ok(scanned);
            }
            for entry in fs::read_dir(session_root)? {
                let workspace_dir = entry?.path();
                if !workspace_dir.is_dir() {
                    continue;
                }
                let Some(workspace_key) = workspace_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                else {
                    continue;
                };
                scan_workspace_dir(&workspace_dir, &workspace_key, &mut scanned)?;
            }
        }
    }
    Ok(scanned)
}

fn scan_workspace_dir(
    dir: &Path,
    expected_workspace_key: &str,
    scanned: &mut Vec<ScannedSession>,
) -> anyhow::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(unit) = SessionUnit::from_path(&path) else {
            continue;
        };
        let Some(id) = unit.id() else {
            continue;
        };
        let transcript = unit.transcript_path();
        let Ok(cwd) = read_session_cwd(&transcript) else {
            continue;
        };
        if workspace_key(&cwd) != expected_workspace_key {
            continue;
        }
        scanned.push(ScannedSession {
            key: (expected_workspace_key.to_owned(), id),
            transcript,
            cwd,
        });
    }
    Ok(())
}
pub(super) fn sync_session_file(
    session_root: &Path,
    cwd: &Path,
    path: &Path,
) -> anyhow::Result<()> {
    let connection = open_index(session_root)?;
    let id = session_id_from_path(path)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))?;
    let workspace_key = workspace_key(cwd);

    // Check staleness under the lock, then release it before the expensive
    // transcript parse so other index operations are not blocked during I/O.
    let (file_size, file_mtime) = session_file_stats(path);
    let needs_sync = {
        let connection = connection
            .lock()
            .expect("session index connection poisoned");
        !indexed_file_is_current(
            &connection,
            &workspace_key,
            &id,
            cwd,
            path,
            file_size,
            file_mtime,
        )?
    };

    if needs_sync {
        let record = summarize_session_file(path, cwd)?;
        anyhow::ensure!(
            record.summary.cwd == cwd,
            "session {} records workspace {}, but is stored under {}",
            record.summary.id,
            record.summary.cwd.display(),
            cwd.display()
        );
        // Re-check metadata under the lock to handle a concurrent writer that
        // changed the file between the staleness check and the parse.
        let (file_size, file_mtime) = session_file_stats(path);
        let connection = connection
            .lock()
            .expect("session index connection poisoned");
        let stale = !indexed_file_is_current(
            &connection,
            &workspace_key,
            &id,
            cwd,
            path,
            file_size,
            file_mtime,
        )?;
        if stale {
            upsert_record(&connection, &workspace_key, &record)?;
        }
    }
    Ok(())
}

pub(super) fn record_created(session: &Session, created_at: u64) -> anyhow::Result<()> {
    let connection = open_index(&session.session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let (file_size, file_mtime) = session_file_stats(&session.path);
    let record = SessionIndexRecord {
        summary: SessionSummary {
            id: session.id.clone(),
            path: session.path.clone(),
            cwd: session.cwd.clone(),
            created_at,
            updated_at: created_at,
            message_count: 0,
            title: None,
            first_user_message: None,
            last_user_message: None,
        },
        file_size,
        file_mtime,
        node_count: 0,
        branch_count: 0,
        active_leaf_id: None,
        effective_format_version: super::persistence::SESSION_VERSION,
    };
    upsert_record(&connection, &session.workspace_key, &record)
}

pub(super) fn set_title(
    session_root: &Path,
    cwd: &Path,
    id_prefix: &str,
    title: &str,
) -> anyhow::Result<()> {
    let paths = matching_session_paths(session_root, cwd, id_prefix)?;
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let workspace_key = workspace_key(cwd);
    match paths.as_slice() {
        [] => anyhow::bail!("no session found matching '{id_prefix}'"),
        [path] => {
            let id = session_id_from_path(path).ok_or_else(|| {
                anyhow::anyhow!("session file has invalid name: {}", path.display())
            })?;
            connection.execute(
                "update sessions set title = ?3 where workspace_key = ?1 and id = ?2",
                params![workspace_key, id, title.trim()],
            )?;
            Ok(())
        }
        _ => anyhow::bail!("multiple sessions match '{id_prefix}'; use a longer UUID prefix"),
    }
}

/// Returns the stored title for an exact session id in a workspace, if any.
pub(super) fn title(session_root: &Path, cwd: &Path, id: &str) -> anyhow::Result<Option<String>> {
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let title = connection
        .query_row(
            "select title from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key(cwd), id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?;
    Ok(title.flatten().filter(|value| !value.is_empty()))
}

/// Sets the title only when the index row still has no title.
///
/// Used by auto-title so a concurrent manual rename (`/title` or
/// `rho sessions rename`) always wins. Returns whether the row was updated.
pub(super) fn set_title_if_absent(
    session_root: &Path,
    cwd: &Path,
    id: &str,
    title: &str,
) -> anyhow::Result<bool> {
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let updated = connection.execute(
        "update sessions
         set title = ?3
         where workspace_key = ?1
           and id = ?2
           and (title is null or title = '')",
        params![workspace_key(cwd), id, title.trim()],
    )?;
    Ok(updated > 0)
}

pub(super) fn record_snapshot_record(
    session: &Session,
    record: &SessionIndexRecord,
) -> anyhow::Result<()> {
    let connection = open_index(&session.session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    upsert_record(&connection, &session.workspace_key, record)
}

fn open_index(session_root: &Path) -> anyhow::Result<Arc<Mutex<Connection>>> {
    let path = session_root.join("index.sqlite3");
    let connections = INDEX_CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut connections = connections.lock().expect("session index cache poisoned");
    if let Some(connection) = connections.get(&path) {
        return Ok(Arc::clone(connection));
    }
    fs::create_dir_all(session_root)?;
    set_private_dir_permissions(session_root)?;
    let mut connection = Connection::open(&path)?;
    set_private_file_permissions(&path)?;
    migrate_index(&mut connection)?;
    let connection = Arc::new(Mutex::new(connection));
    connections.insert(path, Arc::clone(&connection));
    Ok(connection)
}

fn migrate_index(connection: &mut Connection) -> anyhow::Result<()> {
    migrate_index_with_hook(connection, |_| Ok(()))
}

fn migrate_index_with_hook(
    connection: &mut Connection,
    before_commit: impl FnOnce(&Transaction<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let version: u32 = connection.query_row("pragma user_version", [], |row| row.get(0))?;
    if version > INDEX_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported session index schema {version} (maximum supported: {INDEX_SCHEMA_VERSION})"
        );
    }

    let transaction = connection.transaction()?;
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
    ensure_column(&transaction, "node_count integer not null default 0")?;
    ensure_column(&transaction, "branch_count integer not null default 0")?;
    ensure_column(&transaction, "active_leaf_id text")?;
    ensure_column(
        &transaction,
        "effective_format_version integer not null default 1",
    )?;
    if version < 2 {
        // Force the next workspace sync to backfill tree facts for existing rows.
        transaction.execute("update sessions set file_size = null", [])?;
    }
    validate_index_columns(&transaction)?;
    transaction.execute_batch(
        "create index if not exists sessions_workspace_updated_idx
            on sessions(workspace_key, updated_at desc);
         create index if not exists sessions_workspace_id_idx
            on sessions(workspace_key, id);",
    )?;
    transaction.pragma_update(None, "user_version", INDEX_SCHEMA_VERSION)?;
    before_commit(&transaction)?;
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

fn validate_index_columns(connection: &Connection) -> anyhow::Result<()> {
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
        "file_size",
        "file_mtime",
        "node_count",
        "branch_count",
        "active_leaf_id",
        "effective_format_version",
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

fn set_private_file_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct IndexedFile {
    cwd: String,
    path: String,
    file_size: Option<i64>,
    file_mtime: Option<i64>,
    message_count: i64,
    first_user_message: Option<String>,
}

impl IndexedFile {
    fn is_current(&self, path: &Path, file_size: Option<i64>, file_mtime: Option<i64>) -> bool {
        self.path == path.to_string_lossy().as_ref()
            && self.file_size == file_size
            && self.file_mtime == file_mtime
            && (self.message_count == 0 || self.first_user_message.is_some())
    }
}

fn indexed_files_for_scope(
    connection: &Connection,
    scope: &ReconcileScope,
) -> rusqlite::Result<HashMap<IndexKey, IndexedFile>> {
    match scope {
        ReconcileScope::Workspace { workspace_key, .. } => {
            let mut statement = connection.prepare(
                "select workspace_key, id, cwd, path, file_size, file_mtime,
                        message_count, first_user_message
                 from sessions where workspace_key = ?1",
            )?;
            let rows = statement.query_map(params![workspace_key], indexed_file_from_row)?;
            rows.collect()
        }
        ReconcileScope::All => {
            let mut statement = connection.prepare(
                "select workspace_key, id, cwd, path, file_size, file_mtime,
                        message_count, first_user_message
                 from sessions",
            )?;
            let rows = statement.query_map([], indexed_file_from_row)?;
            rows.collect()
        }
    }
}

fn indexed_file_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(IndexKey, IndexedFile)> {
    Ok((
        (row.get(0)?, row.get(1)?),
        IndexedFile {
            cwd: row.get(2)?,
            path: row.get(3)?,
            file_size: row.get(4)?,
            file_mtime: row.get(5)?,
            message_count: row.get(6)?,
            first_user_message: row.get(7)?,
        },
    ))
}

fn indexed_file_is_current(
    connection: &Connection,
    workspace_key: &str,
    id: &str,
    cwd: &Path,
    path: &Path,
    file_size: Option<i64>,
    file_mtime: Option<i64>,
) -> rusqlite::Result<bool> {
    let current = connection
        .query_row(
            "select cwd, path, file_size, file_mtime, message_count, first_user_message
             from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key, id],
            |row| {
                Ok(IndexedFile {
                    cwd: row.get(0)?,
                    path: row.get(1)?,
                    file_size: row.get(2)?,
                    file_mtime: row.get(3)?,
                    message_count: row.get(4)?,
                    first_user_message: row.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(current.is_some_and(|indexed| {
        indexed.cwd == cwd.to_string_lossy() && indexed.is_current(path, file_size, file_mtime)
    }))
}

fn upsert_record(
    connection: &Connection,
    workspace_key: &str,
    record: &SessionIndexRecord,
) -> anyhow::Result<()> {
    let cwd = record.summary.cwd.to_string_lossy().to_string();
    let path = record.summary.path.to_string_lossy().to_string();
    connection.execute(
        "insert into sessions (
            workspace_key,
            cwd,
            id,
            path,
            created_at,
            updated_at,
            message_count,
            title,
            first_user_message,
            last_user_message,
            file_size,
            file_mtime,
            node_count,
            branch_count,
            active_leaf_id,
            effective_format_version
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         on conflict(workspace_key, id) do update set
            cwd = excluded.cwd,
            path = excluded.path,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            message_count = excluded.message_count,
            title = coalesce(sessions.title, excluded.title),
            first_user_message = excluded.first_user_message,
            last_user_message = excluded.last_user_message,
            file_size = excluded.file_size,
            file_mtime = excluded.file_mtime,
            node_count = excluded.node_count,
            branch_count = excluded.branch_count,
            active_leaf_id = excluded.active_leaf_id,
            effective_format_version = excluded.effective_format_version",
        params![
            workspace_key,
            cwd,
            record.summary.id.as_str(),
            path,
            clamp_u64_to_i64(record.summary.created_at),
            clamp_u64_to_i64(record.summary.updated_at),
            clamp_u64_to_i64(record.summary.message_count),
            record.summary.title.as_deref(),
            record.summary.first_user_message.as_deref(),
            record.summary.last_user_message.as_deref(),
            record.file_size,
            record.file_mtime,
            clamp_u64_to_i64(record.node_count),
            clamp_u64_to_i64(record.branch_count),
            record.active_leaf_id.as_deref(),
            record.effective_format_version,
        ],
    )?;
    Ok(())
}

fn stale_index_keys(
    indexed_files: &HashMap<IndexKey, IndexedFile>,
    seen: &HashSet<IndexKey>,
    refreshed: &HashSet<IndexKey>,
) -> Vec<IndexKey> {
    indexed_files
        .iter()
        .filter(|(key, indexed)| {
            !seen.contains(*key)
                || (!Path::new(&indexed.path).exists() && !refreshed.contains(*key))
        })
        .map(|(key, _)| key.clone())
        .collect()
}

fn indexed_record_belongs_to_workspace(
    session_root: &Path,
    key: &IndexKey,
    indexed: &IndexedFile,
) -> bool {
    let expected_dir = session_root.join(&key.0);
    session_dir_in_root(session_root, Path::new(&indexed.cwd)) == expected_dir
        && Path::new(&indexed.path).starts_with(expected_dir)
}

fn transcript_matches_workspace(
    session_root: &Path,
    key: &IndexKey,
    path: &Path,
    expected_cwd: &Path,
) -> bool {
    let expected_dir = session_root.join(&key.0);
    session_dir_in_root(session_root, expected_cwd) == expected_dir
        && path.starts_with(expected_dir)
        && read_session_cwd(path).is_ok_and(|cwd| cwd == expected_cwd)
}

/// Applies upserts and stale deletes in one SQLite transaction.
#[cfg(test)]
fn apply_reconciliation_updates(
    connection: &mut Connection,
    records: &[(IndexKey, SessionIndexRecord)],
    stale_keys: &[IndexKey],
) -> anyhow::Result<()> {
    if records.is_empty() && stale_keys.is_empty() {
        return Ok(());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    apply_reconciliation_transaction(transaction, records, stale_keys)
}

fn apply_reconciliation_transaction(
    transaction: Transaction<'_>,
    records: &[(IndexKey, SessionIndexRecord)],
    stale_keys: &[IndexKey],
) -> anyhow::Result<()> {
    for ((workspace_key, _), record) in records {
        upsert_record(&transaction, workspace_key, record)?;
    }
    for (workspace_key, id) in stale_keys {
        transaction.execute(
            "delete from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key, id],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

/// Drops the index row for a session after its on-disk unit is gone.
pub(super) fn remove_session(
    session_root: &Path,
    workspace_key: &str,
    id: &str,
) -> anyhow::Result<()> {
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    connection.execute(
        "delete from sessions where workspace_key = ?1 and id = ?2",
        params![workspace_key, id],
    )?;
    Ok(())
}

fn summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        cwd: PathBuf::from(row.get::<_, String>(2)?),
        created_at: row.get::<_, i64>(3)?.max(0) as u64,
        updated_at: row.get::<_, i64>(4)?.max(0) as u64,
        message_count: row.get::<_, i64>(5)?.max(0) as u64,
        title: row.get(6)?,
        first_user_message: row.get(7)?,
        last_user_message: row.get(8)?,
    })
}

#[cfg(test)]
#[path = "index_sync_tests.rs"]
mod sync_tests;

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
