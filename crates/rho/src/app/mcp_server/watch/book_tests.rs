use pretty_assertions::assert_eq;
use rho_protocol::{Ask, InputAsker, InputRequest, InputRequestId, JobState, TurnId};

use super::*;
use crate::app::session_control::NewsKey;

fn session(id: &str) -> SessionId {
    SessionId::new(id)
}

fn key(token: &str, id: &str) -> Key {
    (token.to_owned(), session(id))
}

fn row(token: &str, id: &str, error: Option<&str>) -> WatchRecord {
    WatchRecord {
        session: session(id),
        token: token.to_owned(),
        webhook: "hook".to_owned(),
        server: None,
        reported: None,
        error: error.map(str::to_owned),
    }
}

fn book(max_watches: u32, rows: Vec<WatchRecord>) -> (WatchBook, Vec<Start>) {
    WatchBook::resume(
        WatchFile {
            watches: rows,
            ..WatchFile::default()
        },
        max_watches,
    )
}

/// Session `s1` at turn `t1` in `state`, `done` or not, with nothing open.
fn now(state: JobState, done: bool) -> WaitResult {
    WaitResult {
        session: session("s1"),
        state,
        done,
        outcome: None,
        turn: Some(TurnId::new("t1")),
        inputs: Vec::new(),
        cursor: None,
    }
}

/// Session `s1` waiting at turn `t1` for request `id`.
fn asking(id: u64) -> WaitResult {
    WaitResult {
        inputs: vec![InputRequest {
            id: InputRequestId(id),
            ask: Ask::Unknown,
            tool_call: None,
            timeout: None,
            asker: InputAsker::Turn,
        }],
        ..now(JobState::WaitingForInput, true)
    }
}

/// What a `watch` of `s1` by `bot` that read no newer revision starts.
fn started(generation: u64, reported: Option<NewsKey>) -> Result<Watched, OverBudget> {
    Ok(Watched::Started(Start {
        key: key("bot", "s1"),
        generation,
        watching: Watching {
            session: session("s1"),
            webhook: "hook".into(),
            server: None,
            reported,
        },
    }))
}

// Covers: a token past --max-watches is refused naming the budget, its
// limit, and the count asked for; slots held by calls still working count,
// so two calls cannot both take the last one; a renewal of a session the
// token watches takes none; a slot given back frees it; tokens do not share
// a budget.
// Owner: webhook watch budget.
#[test]
fn the_budget_counts_watches_and_held_slots_but_not_renewals() {
    let (mut book, _) = book(2, vec![row("bot", "s1", None)]);
    let fresh = book.reserve("bot", None).unwrap();
    let over = book.reserve("bot", Some(&session("s2")));
    let renewal = book.reserve("bot", Some(&session("s1")));
    let other = book.reserve("ci", Some(&session("s2")));
    book.release(fresh);
    let freed = book.reserve("bot", Some(&session("s2")));

    let slot = |token: &str, id: &str| {
        Ok(Slot {
            token: token.into(),
            session: Some(session(id)),
        })
    };
    assert_eq!(
        (over, renewal, other, freed),
        (
            Err(OverBudget {
                holder: Holder::Token("bot".into()),
                limit: 2,
                asked: 3,
            }),
            slot("bot", "s1"),
            slot("ci", "s2"),
            slot("bot", "s2"),
        )
    );
}

// Covers: a restart resumes every saved watch with its stalled error
// cleared, each as its own generation, and records the server's limit for
// `rho mcp webhook list`.
// Owner: webhook watch resume.
#[test]
fn resuming_clears_stalled_errors_and_starts_every_watch() {
    let (mut book, starts) = book(
        50,
        vec![
            row("bot", "s1", Some("the rho daemon stopped")),
            row("bot", "s2", None),
        ],
    );
    let generations: Vec<(Key, u64)> = starts
        .into_iter()
        .map(|start| (start.key, start.generation))
        .collect();

    assert_eq!(
        (book.snapshot().file, generations),
        (
            WatchFile {
                watches: vec![row("bot", "s1", None), row("bot", "s2", None)],
                max_watches: Some(50),
                ..WatchFile::default()
            },
            vec![(key("bot", "s1"), 1), (key("bot", "s2"), 2)],
        )
    );
}

// Covers: once a renewal replaced a watch, the replaced task's end and
// deliveries change nothing, while the current task's do; `unwatch` of a
// watch already gone changes nothing.
// Owner: webhook watch generations.
#[test]
fn a_replaced_task_changes_nothing() {
    let (mut book, _) = book(50, vec![row("bot", "s1", None)]);
    let slot = book.reserve("bot", Some(&session("s1"))).unwrap();
    let renewed = book.notify(slot, session("s1"), "hook".into(), None);
    let settled = NewsKey {
        state: JobState::Completed,
        turn: Some(TurnId::new("t1")),
        inputs: Vec::new(),
    };
    let delivered = Record::Delivered {
        reported: &settled,
        status: 200,
    };
    let stale = (
        book.apply_record(&renewed.key, 1, delivered, 1_000),
        book.apply_end(&renewed.key, 1, &End::Delivered),
    );
    let after_stale = book.snapshot().file;
    let current = (
        book.apply_record(&renewed.key, 2, delivered, 1_000),
        book.apply_end(&renewed.key, 2, &End::Stalled("refused".into())),
        book.apply_end(&renewed.key, 2, &End::Delivered),
    );
    let after_current = book.snapshot().file;
    let unwatched = (book.remove(&renewed.key), book.remove(&renewed.key));

    let mut stalled = row("bot", "s1", Some("refused"));
    stalled.reported = Some(settled);
    assert_eq!(
        (
            renewed.generation,
            stale,
            after_stale.watches,
            current,
            after_current.watches,
            after_current.deliveries.len(),
            unwatched,
        ),
        (
            2,
            (false, false),
            vec![row("bot", "s1", None)],
            (true, true, false),
            vec![stalled],
            1,
            (true, false),
        )
    );
}

// Covers: `watch` starts from where the session is: the request the call
// itself answered with is not delivered again, but a session waiting with
// nothing open (a request just answered) or still busy is no news, so its
// settle is still delivered; a session settled already is the answer, and
// ends a watch the token had, or, with none, changes nothing and takes no
// slot of a full budget; a new watch past the budget is refused.
// Owner: webhook watch seeding.
#[test]
fn a_watch_starts_from_the_news_its_call_answered() {
    let waiting = NewsKey {
        state: JobState::WaitingForInput,
        turn: Some(TurnId::new("t1")),
        inputs: vec![InputRequestId(3)],
    };
    let cases = [
        (
            "a request open",
            "s1",
            asking(3),
            started(2, Some(waiting)),
            1,
        ),
        (
            "waiting with nothing open",
            "s1",
            now(JobState::WaitingForInput, true),
            started(2, None),
            1,
        ),
        (
            "background work still due",
            "s1",
            now(JobState::Completed, false),
            started(2, None),
            1,
        ),
        (
            "settled",
            "s1",
            now(JobState::Completed, true),
            Ok(Watched::Removed),
            0,
        ),
        (
            "settled, unwatched, at a full budget",
            "s2",
            now(JobState::Completed, true),
            Ok(Watched::Unchanged { watching: false }),
            1,
        ),
        (
            "busy, unwatched, at a full budget",
            "s2",
            now(JobState::Running, false),
            Err(OverBudget {
                holder: Holder::Token("bot".into()),
                limit: 1,
                asked: 2,
            }),
            1,
        ),
    ];
    // Each case's token watches one session, all the budget allows.
    for (name, watched_session, now, expected, watches) in cases {
        let (mut book, _) = book(1, vec![row("bot", watched_session, None)]);
        let read = book.revision(&key("bot", "s1"));
        let result = book.watch("bot", "hook".into(), None, &now, read);
        let after = book.snapshot().file.watches.len();
        assert_eq!((result, after), (expected, watches), "{name}");
    }
}

// Covers: a `watch` that read the watch before a later call set it, as a
// `notify` starting a turn after the `watch` saw the session settled,
// leaves the later call's watch and its task alone, so that turn's settle
// is still delivered.
// Owner: webhook watch revisions.
#[test]
fn a_watch_leaves_a_watch_a_later_call_set() {
    let (mut book, _) = book(50, Vec::new());
    let read = book.revision(&key("bot", "s1"));
    let slot = book.reserve("bot", Some(&session("s1"))).unwrap();
    book.notify(slot, session("s1"), "hook".into(), None);
    let watched = book.watch(
        "bot",
        "hook".into(),
        None,
        &now(JobState::Completed, true),
        read,
    );

    assert_eq!(
        (watched, book.snapshot().file.watches),
        (
            Ok(Watched::Unchanged { watching: true }),
            vec![row("bot", "s1", None)]
        )
    );
}

// A watch read while a resumed task ran, which delivered the settle and
// ended before the call applied, must not start the watch again: the new
// follow would replay that settle and POST it twice.
#[test]
fn a_watch_leaves_a_resumed_watch_that_ended_meanwhile() {
    let (mut book, starts) = book(50, vec![row("bot", "s1", None)]);
    let read = book.revision(&key("bot", "s1"));
    book.apply_end(&key("bot", "s1"), starts[0].generation, &End::Delivered);
    let watched = book.watch(
        "bot",
        "hook".into(),
        None,
        &now(JobState::Running, false),
        read,
    );

    assert_eq!(
        (watched, book.snapshot().file.watches),
        (Ok(Watched::Unchanged { watching: false }), Vec::new())
    );
}
