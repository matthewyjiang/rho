//! Rebuildable derived index. Source transcripts are only ever opened for read.
//! Bootstrap and explicit reconciliation discover files. Normal calls consume
//! catalog invalidations and only stat/reparse changed files.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use rho_sdk::CancellationToken;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    layout::{self, SessionUnit},
    search_evidence::extract,
    search_scope::Workspace,
};

#[derive(Default, Debug, Serialize)]
pub(super) struct Refresh {
    pub reconciled: bool,
    pub files_checked: usize,
    pub files_updated: usize,
    pub bytes_read: u64,
    pub skipped_files: usize,
    pub omitted_records: usize,
}

pub(super) fn open(root: &Path) -> anyhow::Result<Connection> {
    fs::create_dir_all(root)?;
    layout::set_private_dir_permissions(root)?;
    let path = root.join("search.sqlite3");
    for database in [&path, &root.join("index.sqlite3")] {
        anyhow::ensure!(
            !fs::symlink_metadata(database).is_ok_and(|meta| meta.file_type().is_symlink()),
            "refusing a symlinked sessions index"
        );
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    layout::set_private_file_permissions(&file)?;
    let connection = Connection::open(path)?;
    connection.busy_timeout(crate::sqlite_support::BUSY_TIMEOUT)?;
    let version: u32 = connection.query_row("pragma user_version", [], |row| row.get(0))?;
    anyhow::ensure!(
        version <= 1,
        "unsupported sessions search schema {version}; this build supports schema 1"
    );
    connection.execute_batch(
        "pragma foreign_keys=on;
         create table if not exists files (
             key text primary key, path text not null, id text not null,
             cwd text not null, worktree text not null, repo text not null,
             size integer not null, stamp text not null, omitted integer not null);
         create index if not exists files_repo on files(repo);
         create unique index if not exists files_path on files(path);
         create index if not exists files_worktree on files(worktree);
         create table if not exists cursor (identity text not null, seq integer not null);
         create table if not exists pending (path text primary key);
         create table if not exists evidence (
             rowid integer primary key, session text not null references files(key) on delete cascade,
             anchor text not null, role text not null, text text not null,
             omitted integer not null, unique(session, anchor));
         create index if not exists evidence_session_order on evidence(session,rowid);
         create virtual table if not exists evidence_fts using fts5(
             text, content='evidence', content_rowid='rowid', tokenize='porter unicode61');
         create trigger if not exists evidence_insert after insert on evidence begin
             insert into evidence_fts(rowid,text) values(new.rowid,new.text);
         end;
         create trigger if not exists evidence_delete after delete on evidence begin
             insert into evidence_fts(evidence_fts,rowid,text) values('delete',old.rowid,old.text);
         end;
         pragma user_version=1;",
    )?;
    Ok(connection)
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

/// Reconcile changes in one transaction so a failed/cancelled refresh cannot
/// expose a half-built session. Missing files are removed, including FTS rows.
pub(super) fn refresh(
    connection: &mut Connection,
    root: &Path,
    reconcile: bool,
    cancellation: &CancellationToken,
) -> anyhow::Result<Refresh> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let cursor = transaction
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
            transaction
                .prepare("select path from files")?
                .query_map([], |row| row.get::<_, String>(0).map(PathBuf::from))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    paths.extend(
        transaction
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
        let path_string = path.to_string_lossy();
        transaction.execute("delete from pending where path=?1", [&path_string])?;
        report.files_checked += 1;
        if !path.try_exists()? {
            transaction.execute("delete from files where path=?1", [&path_string])?;
            continue;
        }
        let indexed = transaction
            .query_row(
                "select size,stamp from files where path=?1",
                [&path_string],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let stamp = Stamp::read(&path);
        if stamp.as_ref().is_ok_and(|stamp| {
            indexed
                .as_ref()
                .is_some_and(|(size, modified)| *size == stamp.size && *modified == stamp.modified)
        }) {
            continue;
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
            let id = SessionUnit::from_path(&path)
                .and_then(|unit| unit.id())
                .ok_or_else(|| anyhow::anyhow!("invalid session unit"))?;
            index_file(&transaction, root, &path, &id, &stamp?, cancellation)
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
    }
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
    Ok(report)
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

fn index_file(
    connection: &Connection,
    root: &Path,
    path: &Path,
    id: &str,
    stamp: &Stamp,
    cancellation: &CancellationToken,
) -> anyhow::Result<u64> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    let header_bytes = reader.read_line(&mut line)? as u64;
    let header: serde_json::Value = serde_json::from_str(&line)?;
    anyhow::ensure!(header["type"] == "session", "missing session header");
    let version = header["version"].as_u64().unwrap_or(1);
    anyhow::ensure!(
        (1..=u64::from(super::persistence::SESSION_VERSION)).contains(&version),
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
    let workspace = Workspace::resolve(&cwd);
    let path_string = path.to_string_lossy();
    let key = format!("{:x}", Sha256::digest(path_string.as_bytes()));
    connection.execute(
        "insert into files(key,path,id,cwd,worktree,repo,size,stamp,omitted) values(?1,?2,?3,?4,?5,?6,?7,?8,0)",
        params![key, path_string, id, cwd.to_string_lossy(), workspace.worktree.to_string_lossy(), workspace.repo.to_string_lossy(), stamp.size, stamp.modified],
    )?;
    let mut offset = header_bytes;
    let mut omitted = 0;
    let mut insert = connection
        .prepare("insert into evidence(session,anchor,role,text,omitted) values(?1,?2,?3,?4,?5)")?;
    loop {
        anyhow::ensure!(!cancellation.is_cancelled(), "sessions indexing cancelled");
        line.clear();
        let bytes = reader.read_line(&mut line)? as u64;
        if bytes == 0 {
            break;
        }
        if !line.ends_with('\n') {
            omitted += 1;
            break;
        }
        match serde_json::from_str(&line) {
            Ok(record) => {
                for evidence in extract(&record, offset) {
                    insert.execute(params![
                        key,
                        evidence.anchor,
                        evidence.role,
                        evidence.text,
                        evidence.omitted_blocks
                    ])?;
                }
            }
            Err(_) => omitted += 1,
        }
        offset += bytes;
    }
    let after = Stamp::read(path)?;
    anyhow::ensure!(
        after.size == stamp.size && after.modified == stamp.modified,
        "session changed during indexing; retry"
    );
    connection.execute(
        "update files set omitted=?1 where key=?2",
        params![omitted, key],
    )?;
    Ok(offset)
}
