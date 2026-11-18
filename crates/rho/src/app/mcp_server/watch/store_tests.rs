use pretty_assertions::assert_eq;
use rho_protocol::{InputRequestId, JobState, TurnId};
use tempfile::TempDir;

use super::*;

fn file(error: Option<&str>) -> WatchFile {
    WatchFile {
        watches: vec![WatchRecord {
            session: SessionId::new("s1"),
            token: "grok".into(),
            webhook: "grok-routine".into(),
            server: None,
            reported: Some(NewsKey {
                state: JobState::WaitingForInput,
                turn: Some(TurnId::new("t1")),
                inputs: vec![InputRequestId(3)],
            }),
            error: error.map(str::to_owned),
        }],
        deliveries: BTreeMap::from([(
            "grok-routine".to_owned(),
            DeliveryRecord {
                at: 1_794_717_000,
                session: SessionId::new("s1"),
                result: DeliveryResult::Delivered(200),
            },
        )]),
        max_watches: Some(50),
    }
}

// Covers: a restart reads back exactly what the server saved, a rewrite
// replaces the whole file and leaves no temp file beside it, and a missing
// file is no watches.
// Owner: webhook watch store.
#[test]
fn saved_watches_read_back_whole_after_a_rewrite() {
    let root = TempDir::new().unwrap();
    let store = WatchStore::new(root.path().join(DIR));
    let empty = store.load().unwrap();
    store.lock().unwrap().expect("the lock is free");
    store.save(&file(None)).unwrap();
    store.save(&file(Some("the rho daemon stopped"))).unwrap();

    let names: Vec<String> = fs::read_dir(root.path().join(DIR))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(
        (empty, store.load().unwrap(), names),
        (
            WatchFile::default(),
            file(Some("the rho daemon stopped")),
            vec![FILE.to_owned(), LOCK.to_owned()],
        )
    );
}

// Covers: a file that does not parse stops the server naming its path,
// rather than starting with its watches silently dropped.
// Owner: webhook watch store.
#[test]
fn an_unparseable_file_is_refused_naming_its_path() {
    let root = TempDir::new().unwrap();
    let store = WatchStore::new(root.path().to_owned());
    fs::write(store.path(), "{\"watches\": [").unwrap();

    let error = format!("{:#}", store.load().unwrap_err());
    assert!(
        error.starts_with(&format!("{} does not parse", store.path().display())),
        "{error}"
    );
}

// Covers: a second server on the same rho home cannot take the watches
// another holds, so two never deliver the same news.
// Owner: webhook watch store lock.
#[test]
fn one_server_holds_the_watches() {
    let root = TempDir::new().unwrap();
    let store = WatchStore::new(root.path().join(DIR));
    let held = store.lock().unwrap();
    let second = store.lock().unwrap();
    drop(held);
    let after = store.lock().unwrap();

    assert_eq!(
        (second.is_some(), after.is_some()),
        (false, true),
        "the lock is held while its file is open, and free after"
    );
}

// Covers: when two snapshot writes race, an older one finishing last does
// not overwrite the newer one on disk, which holds its changes too.
// Owner: webhook watch store writer.
#[test]
fn the_newest_snapshot_wins() {
    let root = TempDir::new().unwrap();
    let store = WatchStore::new(root.path().join(DIR));
    store.lock().unwrap().expect("the lock is free");
    let writer = Writer::new(store.clone());
    let newer = Snapshot {
        seq: 2,
        file: file(Some("the rho daemon stopped")),
    };
    let older = Snapshot {
        seq: 1,
        file: file(None),
    };
    writer.write_now(&newer).unwrap();
    writer.write_now(&older).unwrap();

    assert_eq!(store.load().unwrap(), newer.file);
}
