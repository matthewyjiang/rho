//! Process-local SQLite connections for the two on-disk model catalogs.
//!
//! Each path has its own mutex: queries and refresh transactions share a connection,
//! but unrelated cache directories never block each other. The closure keeps all
//! statements and transactions inside the lock (and cannot hold it across an await).
//! `Connection` is Send but not Sync; the mutex provides exclusive access across
//! worker threads without reopening and migrating once per thread.
//! Operations must not re-enter the same cache path while holding its connection.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use rusqlite::Connection;

#[derive(Clone, Copy)]
pub(super) enum Schema {
    ProviderModels,
    ModelsDev,
}

pub(super) enum ConnectionState {
    Opened,
    Reused,
}

struct CachedFile {
    connection: Connection,
    identity: Option<FileIdentity>,
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: std::time::SystemTime,
}

impl FileIdentity {
    fn read(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        // An emptied database also needs schema initialization again.
        if metadata.len() == 0 {
            return None;
        }
        Self::from_metadata(&metadata)
    }

    fn from_metadata(metadata: &fs::Metadata) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Some(Self {
                created: metadata.created().ok()?,
            })
        }
    }
}

type SharedConnection = Arc<Mutex<Option<CachedFile>>>;

fn connections() -> &'static Mutex<HashMap<PathBuf, SharedConnection>> {
    static CONNECTIONS: OnceLock<Mutex<HashMap<PathBuf, SharedConnection>>> = OnceLock::new();
    CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

// Fixture teardown must close SQLite handles before removing a temporary directory
// (notably on Windows). Production paths remain cached for the process lifetime.
pub(super) fn forget_connection(path: &Path) {
    connections()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(path);
}

pub(super) fn with_connection<T>(
    path: PathBuf,
    schema: Schema,
    operation: impl FnOnce(&mut Connection, ConnectionState) -> T,
) -> rusqlite::Result<T> {
    let connection = {
        let mut connections = connections()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(connections.entry(path.clone()).or_default())
    };
    // A panicking operation drops its statements/transaction before releasing the
    // mutex, so the connection is still usable after recovering a poisoned lock.
    let mut cached = connection
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let identity = FileIdentity::read(&path);
    let mut state = ConnectionState::Reused;
    if identity.is_none() || cached.as_ref().is_none_or(|file| file.identity != identity) {
        // Drop an unlinked/replaced handle before opening the current path. Failed
        // opens/migrations are not cached; a later lookup can recover normally.
        *cached = None;
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Establish identity before SQLite opens the pathname. Creating an empty
        // file here also gives a new database an identity across the open boundary.
        // If this fails, SQLite still supplies the normal open error below.
        let opened_identity = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .ok()
            .and_then(|file| file.metadata().ok())
            .and_then(|metadata| FileIdentity::from_metadata(&metadata));
        let mut connection = Connection::open(&path)?;
        #[cfg(test)]
        tests::after_open();
        initialize_schema(&mut connection, schema)?;
        // If an external replacement raced open/initialization, do not pin that
        // replacement's identity onto this connection to the previous file.
        let identity = if opened_identity == FileIdentity::read(&path) {
            opened_identity
        } else {
            None
        };
        *cached = Some(CachedFile {
            connection,
            identity,
        });
        state = ConnectionState::Opened;
    }
    Ok(operation(
        &mut cached
            .as_mut()
            .expect("initialized catalog connection")
            .connection,
        state,
    ))
}

fn initialize_schema(connection: &mut Connection, schema: Schema) -> rusqlite::Result<()> {
    // Acquire the writer lock before inspecting columns, so another process's
    // migration cannot race a deferred transaction's read-to-write upgrade.
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let migrations: &[(&str, &[(&str, &str)])] = match schema {
        Schema::ProviderModels => {
            tx.execute_batch(
                "create table if not exists provider_models (
                    provider text not null,
                    model text not null,
                    display_name text not null,
                    context_window integer,
                    max_output_tokens integer,
                    reasoning_capabilities_json text,
                    cache_version integer not null default 1,
                    raw_json text,
                    updated_at integer not null,
                    primary key(provider, model)
                );
                create table if not exists provider_model_refresh (
                    provider text primary key,
                    updated_at integer not null,
                    error text,
                    context_json text
                );
                create table if not exists decision_models (
                    provider text not null,
                    model text not null,
                    updated_at integer not null,
                    primary key(provider, model)
                );",
            )?;
            &[
                (
                    "provider_models",
                    &[
                        ("context_window", "integer"),
                        ("max_output_tokens", "integer"),
                        ("reasoning_capabilities_json", "text"),
                        ("cache_version", "integer not null default 1"),
                    ],
                ),
                ("provider_model_refresh", &[("context_json", "text")]),
            ]
        }
        Schema::ModelsDev => {
            tx.execute_batch(
                "create table if not exists model_metadata (
                    provider text not null,
                    model text not null,
                    metadata_json text not null,
                    updated_at integer not null,
                    cache_version integer not null default 1,
                    primary key(provider, model)
                );
                create table if not exists catalog_snapshot (
                    id integer primary key check (id = 1),
                    cache_version integer not null,
                    updated_at integer not null,
                    -- Borrowed slugs, plus '*' for the shared slug/model tree.
                    borrowed_slugs text not null default ''
                );",
            )?;
            &[
                (
                    "model_metadata",
                    &[("cache_version", "integer not null default 1")],
                ),
                (
                    "catalog_snapshot",
                    &[("borrowed_slugs", "text not null default ''")],
                ),
            ]
        }
    };
    for (table, additions) in migrations {
        let columns = tx
            .prepare("select name from pragma_table_info(?1)")?
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        for (column, declaration) in *additions {
            if !columns.contains(*column) {
                tx.execute_batch(&format!(
                    "alter table {table} add column {column} {declaration}"
                ))?;
            }
        }
    }
    tx.commit()
}

#[cfg(test)]
#[path = "catalog_cache_tests.rs"]
mod tests;
