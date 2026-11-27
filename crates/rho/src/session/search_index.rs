//! Rebuildable derived index. Source transcripts are only ever opened for read.
//! Bootstrap and explicit reconciliation discover files. Normal calls consume
//! catalog invalidations and only stat/reparse changed files. Each file is
//! indexed in its own transaction, so a long bootstrap shows what it has
//! read so far, and a failed or cancelled refresh never a half-read file.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use rho_protocol::SearchLocation;
use rho_sdk::CancellationToken;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::sqlite_support::{OwnerOnlySqlite, ParentDirectoryPrivacy};

use super::{
    layout::{self, SessionUnit},
    persistence::active_display_from_tree,
    search_evidence,
    tree::{ProjectedMessage, SessionTree},
    workspace_scope::Workspace,
    SessionOrigin,
};

/// Names where a client's replay shows each of a session's display rows,
/// one per row (`app::session_events::row_locations`), which the index
/// keeps with each row's evidence. Callers pass it in, so the session store
/// does not reach into the app's replay.
pub(crate) type Locate = fn(&[ProjectedMessage]) -> Vec<Option<SearchLocation>>;

#[cfg(test)]
#[path = "search_index_tests.rs"]
mod tests;

#[derive(Default, Debug, Serialize)]
pub(crate) struct Refresh {
    pub reconciled: bool,
    pub files_checked: usize,
    pub files_updated: usize,
    pub bytes_read: u64,
    pub skipped_files: usize,
    pub omitted_records: usize,
}

/// Opens session root `root`'s search index, creating or rebuilding it
/// when it has no tables of this build's schema.
pub(crate) fn open(root: &Path) -> anyhow::Result<Connection> {
    let path = root.join("search.sqlite3");
    for database in [&path, &root.join("index.sqlite3")] {
        anyhow::ensure!(
            !fs::symlink_metadata(database).is_ok_and(|meta| meta.file_type().is_symlink()),
            "refusing a symlinked sessions index"
        );
    }
    let database = OwnerOnlySqlite::open(path, ParentDirectoryPrivacy::EnforcePrivate, migrate)?;
    // `migrate` enabled the connection-local foreign_keys pragma on this
    // same connection.
    Ok(database.into_connection())
}

const SCHEMA_VERSION: u32 = 6;

#[derive(Debug, thiserror::Error)]
enum OpenError {
    #[error("sessions search I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("sessions search SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unsupported sessions search schema {0}; this build supports schema {SCHEMA_VERSION}")]
    UnsupportedSchema(u32),
}

fn migrate(connection: &mut Connection) -> Result<(), OpenError> {
    connection.pragma_update(None, "foreign_keys", true)?;
    // Versions only rise, so a current one needs no lock: an opener must not
    // wait on a refresh's write, which may hold it while a large transcript
    // loads.
    let version: u32 = connection.query_row("pragma user_version", [], |row| row.get(0))?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    // Serialize the version decision with invalidation. Another opener may have
    // already migrated and rebuilt the cache while this connection was waiting.
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let version: u32 = transaction.query_row("pragma user_version", [], |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(OpenError::UnsupportedSchema(version));
    }
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version > 0 {
        // The index is derived, so an older layout is dropped and rebuilt
        // with current extraction rather than migrated. Version 1 anchors
        // identified positions without binding their contents; version 2
        // indexed the sessions tool's own calls and results; version 3 did
        // not mark the sessions person-facing scopes leave out; version 4
        // held evidence from format 1-3 sessions, which no longer open;
        // version 5 held every branch's evidence, without where replay
        // shows it or when it was saved.
        transaction.execute_batch(
            "drop trigger if exists evidence_insert;
             drop trigger if exists evidence_delete;
             drop table if exists evidence_fts;
             drop table if exists evidence;
             drop table if exists pending;
             drop table if exists cursor;
             drop table if exists files;",
        )?;
    }
    transaction.execute_batch(
        "create table files (
             key text primary key, path text not null, id text not null,
             cwd text not null, worktree text not null, repo text not null,
             size integer not null, stamp text not null, omitted integer not null,
             person_facing integer not null);
         create index files_repo on files(repo);
         create unique index files_path on files(path);
         create index files_worktree on files(worktree);
         create table cursor (identity text not null, seq integer not null);
         create table pending (path text primary key);
         create table evidence (
             rowid integer primary key, session text not null references files(key) on delete cascade,
             anchor text not null, role text not null, text text not null,
             omitted integer not null, at_kind text, at_id text, saved_at integer,
             unique(session, anchor));
         create index evidence_session_order on evidence(session,rowid);
         create virtual table evidence_fts using fts5(
             text, content='evidence', content_rowid='rowid', tokenize='porter unicode61');
         create trigger evidence_insert after insert on evidence begin
             insert into evidence_fts(rowid,text) values(new.rowid,new.text);
         end;
         create trigger evidence_delete after delete on evidence begin
             insert into evidence_fts(evidence_fts,rowid,text) values('delete',old.rowid,old.text);
         end;",
    )?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

#[derive(Debug)]
struct Stamp {
    size: u64,
    modified: String,
}

impl Stamp {
    fn read(path: &Path) -> anyhow::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        anyhow::ensure!(
            metadata.is_file(),
            "session transcript is not a regular file"
        );
        Ok(Self {
            size: metadata.len(),
            modified: metadata
                .modified()?
                .duration_since(UNIX_EPOCH)?
                .as_nanos()
                .to_string(),
        })
    }
}

struct IndexedFile {
    stamp: Stamp,
    cwd: PathBuf,
    workspace: Workspace,
}

/// Retain known scope when the original cwd cannot be resolved. Walking its
/// surviving ancestors could otherwise assign a deleted worktree to another repo.
fn resolve_workspace(cwd: &Path, indexed: Option<&IndexedFile>) -> Workspace {
    if let Some(existing) = cwd.canonicalize().ok().filter(|path| path.is_dir()) {
        return Workspace::resolve(&existing);
    }
    if let Some(indexed) = indexed.filter(|indexed| indexed.cwd == cwd) {
        return indexed.workspace.clone();
    }
    Workspace::resolve(cwd)
}

/// Brings the index up to date with the session catalog's journal, or with
/// every transcript on disk when `reconcile` or when the index has never
/// finished a refresh. Missing files are removed, including FTS rows. Each
/// file commits on its own, and the journal cursor only once every file
/// has, so an interrupted refresh resumes where it stopped: files it
/// indexed keep their stamps and are not read again. After the last
/// commit, a best-effort `VACUUM` reclaims space left by deletes.
pub(crate) fn refresh(
    connection: &mut Connection,
    root: &Path,
    reconcile: bool,
    locate: Locate,
    cancellation: &CancellationToken,
) -> anyhow::Result<Refresh> {
    let cursor = connection
        .query_row("select identity,seq from cursor", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .optional()?;
    let changes = super::search_journal::since(root, cursor.as_ref().map_or(0, |(_, seq)| *seq))?;
    let reconcile = reconcile
        || cursor
            .as_ref()
            .is_none_or(|(identity, seq)| identity != &changes.identity || *seq > changes.through);
    let mut paths: BTreeSet<PathBuf> = if reconcile {
        discover(root)?
    } else {
        changes.paths.into_iter().collect()
    };
    if reconcile {
        // Include absent cached paths so out-of-band deletes are reconciled too.
        paths.extend(
            connection
                .prepare("select path from files")?
                .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    paths.extend(
        connection
            .prepare("select path from pending")?
            .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    );
    let mut report = Refresh {
        reconciled: reconcile,
        ..Refresh::default()
    };
    for path in paths {
        anyhow::ensure!(!cancellation.is_cancelled(), "sessions indexing cancelled");
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        refresh_file(&transaction, root, &path, locate, cancellation, &mut report)?;
        transaction.commit()?;
    }
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    report.omitted_records =
        transaction.query_row("select coalesce(sum(omitted),0) from files", [], |row| {
            row.get(0)
        })?;
    transaction.execute("delete from cursor", [])?;
    transaction.execute(
        "insert into cursor values(?1,?2)",
        params![changes.identity, changes.through],
    )?;
    transaction.commit()?;
    if let Err(error) = vacuum_if_bloated(connection, VACUUM_MIN_FREE_BYTES) {
        // Compaction is an optimization; the committed index is still valid.
        tracing::warn!(%error, "could not vacuum the sessions search index");
    }
    Ok(report)
}

/// Whether a refresh has read every transcript: false before the first
/// finishes, as after a schema change, while what the index holds may miss
/// sessions.
pub(crate) fn complete(connection: &Connection) -> rusqlite::Result<bool> {
    connection.query_row("select exists(select 1 from cursor)", [], |row| row.get(0))
}

/// Brings one transcript's evidence up to date, or drops it when the file is
/// gone. A file that fails to index is left out and retried next refresh.
fn refresh_file(
    transaction: &Transaction<'_>,
    root: &Path,
    path: &Path,
    locate: Locate,
    cancellation: &CancellationToken,
    report: &mut Refresh,
) -> anyhow::Result<()> {
    let path_string = path.to_string_lossy();
    transaction.execute("delete from pending where path=?1", [&path_string])?;
    report.files_checked += 1;
    if !path.try_exists()? {
        transaction.execute("delete from files where path=?1", [&path_string])?;
        return Ok(());
    }
    let indexed = transaction
        .query_row(
            "select size,stamp,cwd,worktree,repo from files where path=?1",
            [&path_string],
            |row| {
                Ok(IndexedFile {
                    stamp: Stamp {
                        size: row.get(0)?,
                        modified: row.get(1)?,
                    },
                    cwd: PathBuf::from(row.get::<_, String>(2)?),
                    workspace: Workspace {
                        worktree: PathBuf::from(row.get::<_, String>(3)?),
                        repo: PathBuf::from(row.get::<_, String>(4)?),
                    },
                })
            },
        )
        .optional()?;
    let stamp = Stamp::read(path);
    if stamp.as_ref().is_ok_and(|stamp| {
        indexed.as_ref().is_some_and(|indexed| {
            indexed.stamp.size == stamp.size && indexed.stamp.modified == stamp.modified
        })
    }) {
        // Git metadata can change independently of the transcript. Refresh
        // scope from the cached cwd without rebuilding evidence or anchors.
        if let Some(indexed) = &indexed {
            let workspace = resolve_workspace(&indexed.cwd, Some(indexed));
            transaction.execute(
                "update files set worktree=?1,repo=?2 where path=?3",
                params![
                    workspace.worktree.to_string_lossy(),
                    workspace.repo.to_string_lossy(),
                    path_string,
                ],
            )?;
        }
        return Ok(());
    }
    transaction.execute("delete from files where path=?1", [&path_string])?;
    let result = (|| {
        // Reject symlinks in the whole session-relative path, including
        // journal entries written by a different process.
        let relative = path.strip_prefix(root)?;
        let mut checked = root.to_path_buf();
        for component in relative.components() {
            anyhow::ensure!(
                matches!(component, std::path::Component::Normal(_)),
                "invalid session path"
            );
            checked.push(component);
            anyhow::ensure!(
                !fs::symlink_metadata(&checked)?.file_type().is_symlink(),
                "symlinked session path"
            );
        }
        let id = SessionUnit::from_path(path)
            .and_then(|unit| unit.id())
            .ok_or_else(|| anyhow::anyhow!("invalid session unit"))?;
        index_file(
            transaction,
            root,
            path,
            &id,
            &stamp?,
            indexed.as_ref(),
            locate,
        )
    })();
    match result {
        Ok(bytes) => {
            report.files_updated += 1;
            report.bytes_read += bytes;
        }
        Err(error) => {
            anyhow::ensure!(!cancellation.is_cancelled(), "{error}");
            transaction.execute("delete from files where path=?1", [&path_string])?;
            transaction.execute(
                "insert or ignore into pending(path) values(?1)",
                [&path_string],
            )?;
            report.skipped_files += 1;
        }
    }
    Ok(())
}

/// Measured on a 331 MiB cache with 165 MiB free: `VACUUM` took ~0.5 s and
/// halved the file. 50 MiB keeps rewrites rare relative to that cost.
const VACUUM_MIN_FREE_BYTES: u64 = 50 * 1024 * 1024;

/// Page accounting for the search cache file.
#[derive(Debug)]
struct Pages {
    size: u64,
    total: u64,
    free: u64,
}

impl Pages {
    fn read(connection: &Connection) -> rusqlite::Result<Self> {
        let pragma = |name| connection.pragma_query_value(None, name, |row| row.get(0));
        Ok(Self {
            size: pragma("page_size")?,
            total: pragma("page_count")?,
            free: pragma("freelist_count")?,
        })
    }

    /// SQLite keeps pages freed by session deletes on its freelist instead of
    /// returning them to the OS. Requiring a quarter of the file to be free
    /// keeps a large, mostly live cache from being rewritten for a sliver.
    fn worth_vacuuming(&self, min_free_bytes: u64) -> bool {
        self.free * self.size >= min_free_bytes && self.free * 4 >= self.total
    }
}

/// Run `VACUUM` once enough of the cache is free, then truncate the WAL so the
/// rewritten pages do not linger beside the shrunken database.
///
/// A concurrent reader can block the truncate. The vacuum is not retried
/// because the freelist is already empty; SQLite finishes shrinking the file
/// when the last connection closes, and search opens one connection per call.
fn vacuum_if_bloated(connection: &Connection, min_free_bytes: u64) -> rusqlite::Result<()> {
    let pages = Pages::read(connection)?;
    if !pages.worth_vacuuming(min_free_bytes) {
        return Ok(());
    }
    connection.execute_batch("vacuum")?;
    let checkpoint_blocked: bool =
        connection.query_row("pragma wal_checkpoint(truncate)", [], |row| row.get(0))?;
    tracing::debug!(
        ?pages,
        checkpoint_blocked,
        "vacuumed the sessions search index"
    );
    Ok(())
}

fn discover(root: &Path) -> anyhow::Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    for workspace in fs::read_dir(root)? {
        let workspace = workspace?;
        if !workspace.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(workspace.path())? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                continue;
            }
            let Some(unit) = SessionUnit::from_path(&entry.path()) else {
                continue;
            };
            paths.insert(unit.transcript_path());
        }
    }
    Ok(paths)
}

/// Indexes the transcript at `path`, as a load of it shows it: the display
/// rows along its active branch, each with where `locate` says replay shows
/// it. A file that does not load is not indexed, as it does not open.
fn index_file(
    connection: &Connection,
    root: &Path,
    path: &Path,
    id: &str,
    stamp: &Stamp,
    indexed: Option<&IndexedFile>,
    locate: Locate,
) -> anyhow::Result<u64> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let header: serde_json::Value = serde_json::from_str(&line)?;
    anyhow::ensure!(header["type"] == "session", "missing session header");
    let version = header["version"].as_u64().unwrap_or(0);
    anyhow::ensure!(
        super::persistence::SUPPORTED_SESSION_VERSIONS.contains(&version),
        "unsupported session format {version}"
    );
    anyhow::ensure!(
        header["id"].as_str() == Some(id),
        "session identity mismatch"
    );
    let cwd = PathBuf::from(
        header["cwd"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing session workspace"))?,
    );
    anyhow::ensure!(
        path.starts_with(layout::session_dir_in_root(root, &cwd)),
        "session workspace mismatch"
    );
    // A header without an origin predates them, so its evidence stays in
    // person-facing scopes, like an unreadable one's.
    let person_facing = serde_json::from_value::<SessionOrigin>(header["origin"].clone())
        .ok()
        .is_none_or(SessionOrigin::is_person_facing);
    let (rows, omitted) = match SessionTree::load(path)
        .and_then(|tree| active_display_from_tree(&tree))
    {
        // A load skips a last record still being written, which the index
        // reports as omitted until it ends.
        Ok(rows) => {
            let torn = torn_tail(&mut reader.into_inner(), stamp.size)?;
            (rows, usize::from(torn))
        }
        // Nor does the session open. Its stamp is kept with no evidence, so
        // it is not read again until it changes, and it is reported omitted.
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "session does not load; not indexed");
            (Vec::new(), 1)
        }
    };
    let workspace = resolve_workspace(&cwd, indexed);
    let path_string = path.to_string_lossy();
    let key = hex::encode(Sha256::digest(path_string.as_bytes()));
    connection.execute(
        "insert into files(key,path,id,cwd,worktree,repo,size,stamp,omitted,person_facing) values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![key, path_string, id, cwd.to_string_lossy(), workspace.worktree.to_string_lossy(), workspace.repo.to_string_lossy(), stamp.size, stamp.modified, omitted, person_facing],
    )?;
    let mut insert = connection.prepare(
        "insert into evidence(session,anchor,role,text,omitted,at_kind,at_id,saved_at)
         values(?1,?2,?3,?4,?5,?6,?7,?8)",
    )?;
    let locations = locate(&rows);
    for evidence in search_evidence::extract(&rows, locations) {
        let (at_kind, at_id) = match &evidence.at {
            Some(SearchLocation::Message { id }) => (Some("message"), Some(id.as_str())),
            Some(SearchLocation::ToolCall { id }) => (Some("tool_call"), Some(id.as_str())),
            Some(_) | None => (None, None),
        };
        insert.execute(params![
            key,
            evidence.anchor,
            evidence.role,
            evidence.text,
            evidence.omitted_blocks,
            at_kind,
            at_id,
            evidence.saved_at,
        ])?;
    }
    let after = Stamp::read(path)?;
    anyhow::ensure!(
        after.size == stamp.size && after.modified == stamp.modified,
        "session changed during indexing; retry"
    );
    Ok(stamp.size)
}

/// Whether the last record of the transcript `file`, `size` bytes long, is
/// torn: unterminated and cut short, as a write still under way leaves it,
/// which a load skips (`SessionTree::load`).
fn torn_tail(file: &mut File, size: u64) -> std::io::Result<bool> {
    const CHUNK: u64 = 64 * 1024;
    let mut tail = Vec::new();
    let mut end = size;
    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        let mut chunk = vec![0; (end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk)?;
        let newline = chunk.iter().rposition(|&byte| byte == b'\n');
        chunk.drain(..newline.map_or(0, |newline| newline + 1));
        chunk.append(&mut tail);
        tail = chunk;
        if newline.is_some() {
            break;
        }
        end = start;
    }
    Ok(!tail.trim_ascii().is_empty()
        && serde_json::from_slice::<serde::de::IgnoredAny>(&tail)
            .is_err_and(|error| error.is_eof()))
}
