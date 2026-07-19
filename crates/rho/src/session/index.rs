use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use rusqlite::{params, Connection, OptionalExtension, Transaction};

const INDEX_SCHEMA_VERSION: u32 = 1;

static INDEX_CONNECTIONS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>> =
    OnceLock::new();

#[cfg(test)]
use rho_providers::model::Message;

use super::{
    clamp_u64_to_i64, session_dir_in_root, session_file_stats, session_id_from_path,
    set_private_dir_permissions, summarize_session_file, workspace_key, Session,
    SessionIndexRecord, SessionSummary,
};

#[cfg(test)]
use super::{unix_timestamp_secs, user_message_text};

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
    let workspace_key = workspace_key(cwd);
    let mut statement = connection.prepare(
        "select path
         from sessions
         where workspace_key = ?1 and substr(id, 1, length(?2)) = ?2
         order by id asc",
    )?;
    let rows = statement.query_map(params![workspace_key, id_prefix], |row| {
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
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let workspace_key = workspace_key(cwd);
    let dir = session_dir_in_root(session_root, cwd);
    let mut seen = HashSet::new();

    if dir.exists() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let Some(id) = session_id_from_path(&path) else {
                continue;
            };
            seen.insert(id.clone());
            let (file_size, file_mtime) = session_file_stats(&path);
            if indexed_file_is_current(
                &connection,
                &workspace_key,
                &id,
                &path,
                file_size,
                file_mtime,
            )? {
                continue;
            }
            if let Ok(record) = summarize_session_file(&path, cwd) {
                upsert_record(&connection, &workspace_key, &record)?;
            }
        }
    }

    remove_stale_records(&connection, &workspace_key, &seen)?;
    Ok(())
}

pub(super) fn sync_session_file(
    session_root: &Path,
    cwd: &Path,
    path: &Path,
) -> anyhow::Result<()> {
    let connection = open_index(session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let id = session_id_from_path(path)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))?;
    let workspace_key = workspace_key(cwd);
    let (file_size, file_mtime) = session_file_stats(path);
    if !indexed_file_is_current(
        &connection,
        &workspace_key,
        &id,
        path,
        file_size,
        file_mtime,
    )? {
        let record = summarize_session_file(path, cwd)?;
        upsert_record(&connection, &workspace_key, &record)?;
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

#[cfg(test)]
pub(super) fn record_message(session: &Session, message: &Message) -> anyhow::Result<()> {
    let connection = open_index(&session.session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let updated_at = clamp_u64_to_i64(unix_timestamp_secs());
    let user_message = user_message_text(message);
    let (file_size, file_mtime) = session_file_stats(&session.path);
    let rows = connection.execute(
        "update sessions
         set updated_at = max(updated_at, ?3),
             message_count = message_count + 1,
             first_user_message = coalesce(first_user_message, ?4),
             last_user_message = coalesce(?4, last_user_message),
             file_size = ?5,
             file_mtime = ?6
         where workspace_key = ?1 and id = ?2",
        params![
            session.workspace_key.as_str(),
            session.id.as_str(),
            updated_at,
            user_message,
            file_size,
            file_mtime
        ],
    )?;
    if rows == 0 {
        let record = summarize_session_file(&session.path, &session.cwd)?;
        upsert_record(&connection, &session.workspace_key, &record)?;
    }
    Ok(())
}

pub(super) fn record_snapshot(session: &Session) -> anyhow::Result<()> {
    let connection = open_index(&session.session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let record = summarize_session_file(&session.path, &session.cwd)?;
    upsert_record(&connection, &session.workspace_key, &record)
}

#[cfg(test)]
pub(super) fn record_replaced(session: &Session) -> anyhow::Result<()> {
    let connection = open_index(&session.session_root)?;
    let connection = connection
        .lock()
        .expect("session index connection poisoned");
    let record = summarize_session_file(&session.path, &session.cwd)?;
    upsert_record(&connection, &session.workspace_key, &record)
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
            primary key (workspace_key, id)
        );",
        )?;
        ensure_column(&transaction, "title text")?;
        ensure_column(&transaction, "first_user_message text")?;
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
    ];
    let mut statement = connection.prepare("pragma table_info(sessions)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    let missing = REQUIRED_COLUMNS
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
    let mut statement = connection.prepare("pragma table_info(sessions)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    let exists = columns
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|column| column == column_name);
    if !exists {
        connection.execute(
            &format!("alter table sessions add column {column_definition}"),
            [],
        )?;
    }
    Ok(())
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

fn indexed_file_is_current(
    connection: &Connection,
    workspace_key: &str,
    id: &str,
    path: &Path,
    file_size: Option<i64>,
    file_mtime: Option<i64>,
) -> rusqlite::Result<bool> {
    let current = connection
        .query_row(
            "select path, file_size, file_mtime, message_count, first_user_message
             from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    Ok(current.is_some_and(
        |(indexed_path, indexed_size, indexed_mtime, message_count, first_user_message)| {
            indexed_path == path.to_string_lossy().as_ref()
                && indexed_size == file_size
                && indexed_mtime == file_mtime
                && (message_count == 0 || first_user_message.is_some())
        },
    ))
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
            file_mtime
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
            file_mtime = excluded.file_mtime",
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
        ],
    )?;
    Ok(())
}

fn remove_stale_records(
    connection: &Connection,
    workspace_key: &str,
    seen: &HashSet<String>,
) -> anyhow::Result<()> {
    let mut statement =
        connection.prepare("select id, path from sessions where workspace_key = ?1")?;
    let rows = statement.query_map(params![workspace_key], |row| {
        Ok((
            row.get::<_, String>(0)?,
            PathBuf::from(row.get::<_, String>(1)?),
        ))
    })?;
    let stale_ids = rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|(id, path)| (!seen.contains(&id) || !path.exists()).then_some(id))
        .collect::<Vec<_>>();

    for id in stale_ids {
        connection.execute(
            "delete from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key, id],
        )?;
    }
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    const INDEX_V0: &str = include_str!("fixtures/index-v0.sql");
    const INDEX_V1: &str = include_str!("fixtures/index-v1.sql");

    #[test]
    fn every_supported_index_fixture_migrates_transactionally() {
        for (source_version, fixture) in [(0, INDEX_V0), (1, INDEX_V1)] {
            let mut connection = Connection::open_in_memory().unwrap();
            connection.execute_batch(fixture).unwrap();
            let before: u32 = connection
                .query_row("pragma user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(before, source_version);

            migrate_index(&mut connection).unwrap();

            let after: u32 = connection
                .query_row("pragma user_version", [], |row| row.get(0))
                .unwrap();
            let title: Option<String> = connection
                .query_row(
                    "select title from sessions where id = 'fixture-session'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(after, INDEX_SCHEMA_VERSION);
            if source_version == 1 {
                assert_eq!(title.as_deref(), Some("fixture title"));
            }
            validate_index_columns(&connection).unwrap();
        }
    }

    #[test]
    fn rejects_newer_and_malformed_index_schemas() {
        let mut newer = Connection::open_in_memory().unwrap();
        newer
            .pragma_update(None, "user_version", INDEX_SCHEMA_VERSION + 1)
            .unwrap();
        let error = migrate_index(&mut newer).unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported session index schema"));

        let mut malformed = Connection::open_in_memory().unwrap();
        malformed
            .execute_batch(
                "pragma user_version = 1;
                 create table sessions (workspace_key text not null);",
            )
            .unwrap();
        let error = migrate_index(&mut malformed).unwrap_err();
        assert!(error.to_string().contains("malformed session index schema"));
    }

    #[test]
    fn failed_index_migration_rolls_back_every_schema_change() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(INDEX_V0).unwrap();

        let error = migrate_index_with_hook(&mut connection, |_| {
            anyhow::bail!("injected migration failure")
        })
        .unwrap_err();

        assert!(error.to_string().contains("injected migration failure"));
        let version: u32 = connection
            .query_row("pragma user_version", [], |row| row.get(0))
            .unwrap();
        let mut statement = connection.prepare("pragma table_info(sessions)").unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(version, 0);
        assert!(!columns.iter().any(|column| column == "title"));
        assert!(!columns.iter().any(|column| column == "first_user_message"));
    }

    #[test]
    fn open_index_creates_schema() {
        let root = TempDir::new().unwrap();
        let connection = open_index(root.path()).unwrap();
        let connection = connection.lock().unwrap();

        let table_count: i64 = connection
            .query_row(
                "select count(*) from sqlite_master where type = 'table' and name = 'sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(table_count, 1);
        let version: u32 = connection
            .query_row("pragma user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, INDEX_SCHEMA_VERSION);
        assert!(root.path().join("index.sqlite3").exists());
    }
}
