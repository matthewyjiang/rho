use super::*;
use pretty_assertions::assert_eq;
#[cfg(unix)]
use rusqlite::OptionalExtension;

thread_local! {
    static AFTER_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn after_open() {
    let hook = AFTER_OPEN.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

// Covers: replacement during open must not permanently label the old handle
// with the new file's identity. Owner: SQLite filesystem boundary.
#[cfg(unix)]
#[test]
fn replacement_during_open_is_seen_on_the_next_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite3");
    let replacement = dir.path().join("replacement.sqlite3");
    for (file, value) in [(&path, 1), (&replacement, 2)] {
        let mut connection = Connection::open(file).unwrap();
        initialize_schema(&mut connection, Schema::ProviderModels).unwrap();
        connection
            .execute(
                "insert into provider_model_refresh (provider, updated_at) values ('probe', ?1)",
                [value],
            )
            .unwrap();
    }
    let destination = path.clone();
    AFTER_OPEN.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            fs::rename(replacement, destination).unwrap()
        }));
    });
    let read = || {
        with_connection(path.clone(), Schema::ProviderModels, |connection, _| {
            connection.query_row(
                "select updated_at from provider_model_refresh where provider = 'probe'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .and_then(std::convert::identity)
    };
    // The racing lookup may read the old file or report SQLite's moved-file
    // error. Neither outcome may pin the old connection for subsequent reads.
    let _ = read();
    assert_eq!(read().unwrap(), 2);
    forget_connection(&path);
}

// Covers: clearing or atomically replacing a cache must not keep serving the
// unlinked file through a reused connection; the next refresh must still work.
// Owner: catalog filesystem/connection boundary (Unix permits replacing open files).
#[cfg(unix)]
#[test]
fn catalog_connections_follow_deleted_replaced_and_truncated_files() {
    #[derive(Debug)]
    enum Change {
        Delete,
        Replace,
        Truncate,
    }
    for (schema, insert, select) in [
        (
            Schema::ProviderModels,
            "insert into provider_model_refresh (provider, updated_at) values ('probe', ?1)",
            "select updated_at from provider_model_refresh where provider = 'probe'",
        ),
        (
            Schema::ModelsDev,
            "insert into catalog_snapshot (id, cache_version, updated_at) values (1, 1, ?1)",
            "select updated_at from catalog_snapshot where id = 1",
        ),
    ] {
        for change in [Change::Delete, Change::Replace, Change::Truncate] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("catalog.sqlite3");
            let read = || {
                with_connection(path.clone(), schema, |connection, _| {
                    connection
                        .query_row(select, [], |row| row.get::<_, i64>(0))
                        .optional()
                })
                .unwrap()
                .unwrap()
            };
            with_connection(path.clone(), schema, |connection, _| {
                connection.execute(insert, [1]).unwrap();
            })
            .unwrap();
            assert_eq!(read(), Some(1));

            let expected = match change {
                Change::Delete => {
                    fs::remove_file(&path).unwrap();
                    None
                }
                Change::Replace => {
                    let replacement = dir.path().join("replacement.sqlite3");
                    let mut connection = Connection::open(&replacement).unwrap();
                    initialize_schema(&mut connection, schema).unwrap();
                    connection.execute(insert, [2]).unwrap();
                    drop(connection);
                    fs::rename(replacement, &path).unwrap();
                    Some(2)
                }
                Change::Truncate => {
                    fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_len(0)
                        .unwrap();
                    None
                }
            };
            assert_eq!(read(), expected, "{change:?}");
            with_connection(path.clone(), schema, |connection, _| {
                connection.execute_batch(&format!(
                    "delete from {};",
                    match schema {
                        Schema::ProviderModels => "provider_model_refresh",
                        Schema::ModelsDev => "catalog_snapshot",
                    }
                ))?;
                connection.execute(insert, [3])
            })
            .unwrap()
            .unwrap();
            assert_eq!(read(), Some(3));
            forget_connection(&path);
        }
    }
}

// Covers: simultaneous refreshes share one cache without losing committed rows,
// and a different fixture path does not inherit that cache's contents.
// Owner: catalog connection synchronization.
#[test]
fn concurrent_refreshes_and_distinct_paths_stay_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.sqlite3");
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for provider in ["first", "second"] {
            let path = &path;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                with_connection(path.clone(), Schema::ProviderModels, |connection, _| {
                    let tx = connection.transaction()?;
                    tx.execute(
                        "insert into provider_model_refresh (provider, updated_at) values (?1, 1)",
                        [provider],
                    )?;
                    tx.commit()
                })
                .unwrap()
                .unwrap();
            });
        }
    });
    let other = dir.path().join("other.sqlite3");
    for (path, expected) in [
        (&path, vec!["first".to_string(), "second".to_string()]),
        (&other, vec![]),
    ] {
        let providers = with_connection(path.clone(), Schema::ProviderModels, |connection, _| {
            connection
                .prepare("select provider from provider_model_refresh order by provider")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .unwrap();
        assert_eq!(providers, expected);
        forget_connection(path);
    }
}

// Covers: pre-versioned models.dev databases remain readable and refreshable
// after connection initialization migrates both legacy tables.
// Owner: models.dev SQLite migration; legacy provider tables have existing coverage.
#[test]
fn legacy_models_dev_schema_retains_rows_and_accepts_current_refreshes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "create table model_metadata (
            provider text not null, model text not null, metadata_json text not null,
            updated_at integer not null, primary key(provider, model)
        );
        create table catalog_snapshot (
            id integer primary key, cache_version integer not null, updated_at integer not null
        );
        insert into model_metadata values ('probe', 'old', '{}', 1);
        insert into catalog_snapshot values (1, 1, 1);",
        )
        .unwrap();
    drop(connection);
    let row = with_connection(path.clone(), Schema::ModelsDev, |connection, _| {
        connection.execute("update catalog_snapshot set borrowed_slugs = '*'", [])?;
        connection.query_row(
            "select metadata_json, model_metadata.cache_version, borrowed_slugs
             from model_metadata cross join catalog_snapshot",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
    })
    .unwrap()
    .unwrap();
    assert_eq!(row, ("{}".to_string(), 1, "*".to_string()));
    forget_connection(&path);
}
