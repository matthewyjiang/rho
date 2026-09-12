//! Persistent invalidation from the existing session catalog. Triggers cover
//! every writer, including processes that opened their catalog before search.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

pub(super) fn install(connection: &Connection) -> rusqlite::Result<()> {
    // Do not use INSERT OR REPLACE inside these triggers: an outer catalog
    // UPSERT overrides its conflict policy and can fail on the unique path.
    connection.execute_batch(
        "create table if not exists search_changes (
             seq integer primary key autoincrement, path text not null unique);
         create table if not exists search_journal_identity (id text primary key);
         insert into search_journal_identity select hex(randomblob(16))
             where not exists(select 1 from search_journal_identity);
         create trigger if not exists search_session_insert after insert on sessions begin
             delete from search_changes where path=new.path;
             insert into search_changes(path) values(new.path);
         end;
         create trigger if not exists search_session_update after update on sessions begin
             delete from search_changes where path in (old.path,new.path);
             insert into search_changes(path) select old.path where old.path<>new.path;
             insert into search_changes(path) values(new.path);
         end;
         create trigger if not exists search_session_delete after delete on sessions begin
             delete from search_changes where path=old.path;
             insert into search_changes(path) values(old.path);
         end;",
    )
}

pub(super) struct Changes {
    pub identity: String,
    pub through: i64,
    pub paths: Vec<PathBuf>,
}

pub(super) fn since(root: &Path, cursor: i64) -> anyhow::Result<Changes> {
    let connection = super::index::open_index(root)?;
    let mut connection = connection
        .lock()
        .expect("session index connection poisoned");
    let transaction = connection.transaction()?;
    let identity = transaction.query_row("select id from search_journal_identity", [], |row| {
        row.get(0)
    })?;
    let through = transaction.query_row(
        "select coalesce(max(seq),0) from search_changes",
        [],
        |row| row.get(0),
    )?;
    let paths = transaction
        .prepare("select path from search_changes where seq>?1 and seq<=?2 order by seq")?
        .query_map(params![cursor, through], |row| {
            row.get::<_, String>(0).map(PathBuf::from)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    transaction.commit()?;
    Ok(Changes {
        identity,
        through,
        paths,
    })
}
