use std::{fs, path::Path};

use pretty_assertions::assert_eq;
use rho_sdk::SessionId;
use tempfile::TempDir;

use super::PullRequestStore;
use crate::app::pull_requests::{
    model::{PrId, RepoId},
    record::{Link, LinkOrigin, SessionRecord},
    state::{End, News, WatchState},
};

fn pr(number: u32) -> PrId {
    PrId {
        repo: RepoId {
            host: "github.com".to_owned(),
            owner: "acme".to_owned(),
            name: "widgets".to_owned(),
        },
        number,
    }
}

fn url(number: u32) -> String {
    format!("https://github.com/acme/widgets/pull/{number}")
}

/// News about PR 12, the `seq`th queued in its record: queue it in seq
/// order, from 0, for it to be queued as itself.
fn news(seq: u64, end: Option<End>) -> News {
    News {
        seq,
        pr: pr(12),
        url: url(12),
        head: Some("1a2b3c4".to_owned()),
        changes: Vec::new(),
        end,
        at: 1_000 + seq,
        pushed: false,
        checks: None,
    }
}

fn update(seq: u64) -> News {
    news(seq, None)
}

fn closed(seq: u64) -> News {
    news(seq, Some(End::Closed))
}

/// A record with a watched link, an unwatched one, and news of which one
/// was dropped.
fn full_record() -> SessionRecord {
    let mut record = SessionRecord::default();
    record.link(pr(12), url(12), LinkOrigin::Agent, 100);
    record.link(pr(9), url(9), LinkOrigin::User, 200);
    record.link_mut(&pr(12)).unwrap().watch = Some(WatchState::new(100));
    for item in [update(0), update(1), closed(2)] {
        record.queue_news(item, 2, &[]);
    }
    record
}

/// The names of the files in `dir`, sorted.
fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn sorted(mut records: Vec<(SessionId, SessionRecord)>) -> Vec<(SessionId, SessionRecord)> {
    records.sort_by(|a, b| a.0.cmp(&b.0));
    records
}

// Covers: a whole record (links, watch state, news, drop count) written and
// loaded back by a new store is the same record under the same session, and
// a rewrite replaces the file rather than adding one.
// Owner: PR record store.
#[test]
fn records_round_trip_and_rewrites_replace_them() {
    let home = TempDir::new().unwrap();
    let store = PullRequestStore::open(home.path()).unwrap();
    let first = SessionId::new();
    let second = SessionId::new();
    let mut linked_only = SessionRecord::default();
    linked_only.link(pr(3), url(3), LinkOrigin::User, 50);
    store.save(&first, &linked_only).unwrap();
    store.save(&second, &linked_only).unwrap();
    let full = full_record();
    store.save(&first, &full).unwrap();

    let reopened = PullRequestStore::open(home.path()).unwrap();
    assert_eq!(
        sorted(reopened.load_all()),
        sorted(vec![(first, full), (second, linked_only)])
    );
}

// Covers: files that are not a current record named for its own session
// (torn, foreign JSON, a newer format version, named for another session,
// named for no session id) are skipped without hiding the valid record, and
// files that are not record files (an interrupted write's temp file, a
// stray note) are ignored.
// Owner: PR record store.
#[test]
fn unreadable_record_files_are_skipped() {
    let home = TempDir::new().unwrap();
    let store = PullRequestStore::open(home.path()).unwrap();
    let kept = SessionId::new();
    store.save(&kept, &full_record()).unwrap();
    let dir = home.path().join("pull-requests");
    let saved = fs::read(dir.join(format!("{kept}.json"))).unwrap();
    let newer =
        String::from_utf8(saved.clone())
            .unwrap()
            .replacen("\"version\": 1", "\"version\": 2", 1);
    for (name, contents) in [
        (
            format!("{}.json", SessionId::new()),
            saved[..saved.len() / 2].to_vec(),
        ),
        (
            format!("{}.json", SessionId::new()),
            b"{\"hello\": 1}".to_vec(),
        ),
        (format!("{}.json", SessionId::new()), newer.into_bytes()),
        (format!("{}.json", SessionId::new()), saved.clone()),
        (".hidden.json".to_owned(), saved.clone()),
        (format!(".{kept}.json.0123.tmp"), b"{".to_vec()),
        ("notes.txt".to_owned(), b"hello".to_vec()),
    ] {
        fs::write(dir.join(name), contents).unwrap();
    }

    assert_eq!(store.load_all(), vec![(kept, full_record())]);
}

// Covers: saving a record that has become empty deletes its file, while a
// record left with only undelivered news keeps it; forgetting a session
// deletes its file, and forgetting it again succeeds.
// Owner: PR record store.
#[test]
fn empty_and_forgotten_records_leave_no_file() {
    let home = TempDir::new().unwrap();
    let store = PullRequestStore::open(home.path()).unwrap();
    let dir = home.path().join("pull-requests");
    let emptied = SessionId::new();
    let forgotten = SessionId::new();
    store.save(&forgotten, &full_record()).unwrap();

    let mut record = SessionRecord::default();
    record.link(pr(12), url(12), LinkOrigin::User, 1);
    record.queue_news(update(0), 50, &[]);
    record.unlink(&pr(12));
    store.save(&emptied, &record).unwrap();
    let mut both = vec![format!("{emptied}.json"), format!("{forgotten}.json")];
    both.sort();
    assert_eq!(files(&dir), both);

    record.acknowledge(&[0]);
    store.save(&emptied, &record).unwrap();
    store.forget(&forgotten).unwrap();
    store.forget(&forgotten).unwrap();
    assert_eq!(files(&dir), Vec::<String>::new());
}

// Covers: a session id that is not a plain file name cannot make the store
// write, or delete, outside its directory or a hidden file inside it.
// Owner: PR record store.
#[test]
fn ids_that_are_not_plain_file_names_are_refused() {
    let home = TempDir::new().unwrap();
    let store = PullRequestStore::open(home.path()).unwrap();
    for id in ["../escape", "a/b", "a\\b", ".hidden", "x.json"] {
        let session = SessionId::from_string(id).unwrap();
        let saved = store.save(&session, &full_record()).map_err(|e| e.kind());
        let forgotten = store.forget(&session).map_err(|e| e.kind());
        assert_eq!(
            (id, saved, forgotten),
            (
                id,
                Err(std::io::ErrorKind::InvalidInput),
                Err(std::io::ErrorKind::InvalidInput)
            )
        );
    }
    assert_eq!(files(home.path()), vec!["pull-requests".to_owned()]);
    assert_eq!(
        files(&home.path().join("pull-requests")),
        Vec::<String>::new()
    );
}

// Covers: past the cap the oldest news that is neither an end nor in
// flight is dropped and counted, ends are never dropped, news a turn is
// delivering is never dropped (nor counted, as it may be told), and a new
// update is the one dropped when everything older is an end.
// Owner: PR session record.
#[test]
fn queued_news_past_the_cap_drops_the_oldest_update() {
    struct Case {
        name: &'static str,
        cap: usize,
        queued: Vec<News>,
        in_flight: Vec<u64>,
        kept: Vec<News>,
        dropped: u32,
    }
    let cases = [
        Case {
            name: "under the cap",
            cap: 3,
            queued: vec![update(0), update(1), update(2)],
            in_flight: Vec::new(),
            kept: vec![update(0), update(1), update(2)],
            dropped: 0,
        },
        Case {
            name: "each item past the cap drops the oldest",
            cap: 2,
            queued: vec![update(0), update(1), update(2), update(3)],
            in_flight: Vec::new(),
            kept: vec![update(2), update(3)],
            dropped: 2,
        },
        Case {
            name: "an older end is passed over",
            cap: 2,
            queued: vec![closed(0), update(1), update(2)],
            in_flight: Vec::new(),
            kept: vec![closed(0), update(2)],
            dropped: 1,
        },
        Case {
            name: "news in flight is passed over",
            cap: 2,
            queued: vec![update(0), update(1), update(2)],
            in_flight: vec![0],
            kept: vec![update(0), update(2)],
            dropped: 1,
        },
        Case {
            name: "ends alone may pass the cap",
            cap: 1,
            queued: vec![closed(0), closed(1)],
            in_flight: Vec::new(),
            kept: vec![closed(0), closed(1)],
            dropped: 0,
        },
        Case {
            name: "a new update behind only ends is dropped",
            cap: 2,
            queued: vec![closed(0), closed(1), update(2)],
            in_flight: Vec::new(),
            kept: vec![closed(0), closed(1)],
            dropped: 1,
        },
    ];
    for case in cases {
        let mut record = SessionRecord::default();
        for item in case.queued {
            record.queue_news(item, case.cap, &case.in_flight);
        }
        assert_eq!(
            (case.name, record.news(), record.news_dropped()),
            (case.name, case.kept.as_slice(), case.dropped)
        );
    }
}

// Covers: acknowledging removes exactly the delivered seqs, an identical
// item queued apart staying, and ignores seqs no longer queued; the drop
// count clears only when the queue empties, since news queued during
// delivery was not told of.
// Owner: PR session record.
#[test]
fn acknowledged_news_leaves_the_queue() {
    struct Case {
        name: &'static str,
        delivered: Vec<u64>,
        kept: Vec<News>,
        dropped: u32,
    }
    // Queued with a cap of 4: update(0) is dropped. Updates 1 and 2 are
    // the same news found twice.
    let twice = |seq| News {
        at: 1_001,
        ..update(seq)
    };
    let queued = [update(0), twice(1), twice(2), closed(3), update(4)];
    let cases = [
        Case {
            name: "part of the queue keeps the drop count",
            delivered: vec![1, 3],
            kept: vec![twice(2), update(4)],
            dropped: 1,
        },
        Case {
            name: "news no longer queued is ignored",
            delivered: vec![0, 9],
            kept: vec![twice(1), twice(2), closed(3), update(4)],
            dropped: 1,
        },
        Case {
            name: "the whole queue clears the drop count",
            delivered: vec![4, 2, 3, 1],
            kept: Vec::new(),
            dropped: 0,
        },
    ];
    for case in cases {
        let mut record = SessionRecord::default();
        for item in queued.clone() {
            record.queue_news(item, 4, &[]);
        }
        record.acknowledge(&case.delivered);
        assert_eq!(
            (case.name, record.news(), record.news_dropped()),
            (case.name, case.kept.as_slice(), case.dropped)
        );
    }
}

// Covers: linking a linked PR changes nothing (the first origin and time
// stand), unlinking reports whether it was linked and ends the watch, and
// only watched links are watching.
// Owner: PR session record.
#[test]
fn links_are_idempotent_and_unlinking_ends_the_watch() {
    let mut record = SessionRecord::default();
    let linked = [
        record.link(pr(12), url(12), LinkOrigin::User, 1),
        record.link(pr(12), url(12), LinkOrigin::Agent, 2),
        record.link(pr(9), url(9), LinkOrigin::Agent, 3),
    ];
    record.link_mut(&pr(9)).unwrap().watch = Some(WatchState::new(3));
    let unwatched = Link {
        pr: pr(12),
        url: url(12),
        origin: LinkOrigin::User,
        linked_at: 1,
        watch: None,
        told_head: None,
        stood: None,
    };
    let watched = Link {
        pr: pr(9),
        url: url(9),
        origin: LinkOrigin::Agent,
        linked_at: 3,
        watch: Some(WatchState::new(3)),
        told_head: None,
        stood: None,
    };
    assert_eq!(linked, [true, false, true]);
    assert_eq!(record.links(), [unwatched.clone(), watched.clone()]);
    assert_eq!(
        record.watching().collect::<Vec<_>>(),
        vec![(&watched, &WatchState::new(3))]
    );

    let unlinked = [record.unlink(&pr(9)), record.unlink(&pr(9))];
    assert_eq!(unlinked, [true, false]);
    assert_eq!(record.links(), [unwatched]);
    assert_eq!(record.watching().count(), 0);
}
