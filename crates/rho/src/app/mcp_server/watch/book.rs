//! The watches' bookkeeping, apart from the tasks and the file that carry
//! it out: which watches there are, the budget slots calls hold while they
//! work, the task generation that runs each watch, the revision a call
//! reads to tell whether another call decided a watch while it awaited,
//! and what a finished task or a delivery changes. [`Watches`](super::Watches) keeps one book
//! under its mutex, spawns and aborts the tasks the book names, and writes
//! the snapshots it takes.

use std::collections::{HashMap, HashSet};

use rho_protocol::SessionId;

use super::{
    store::{DeliveryRecord, DeliveryResult, Snapshot, WatchFile, WatchRecord},
    watcher::{End, News, Record, Watching},
};
use crate::app::session_control::{Holder, WaitResult};

/// A watch's key: one per token and session.
pub(super) type Key = (String, SessionId);

/// A watch past its token's budget: the budget, its limit, and the count
/// asked for.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("session watches budget for {holder}: limit {limit}, asked {asked}")]
pub(crate) struct OverBudget {
    pub(crate) holder: Holder,
    pub(crate) limit: u32,
    pub(crate) asked: u32,
}

/// A slot of a token's budget that [`WatchBook::reserve`] holds for a
/// `notify` call while the core starts its turn, until the call starts its
/// watch ([`WatchBook::notify`]) or gives the slot back
/// ([`WatchBook::release`]).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Slot {
    token: String,
    /// The session the call watches; `None` for one `new` has not made yet.
    session: Option<SessionId>,
}

/// A watch to run as a task: the generation the task runs as, and what it
/// watches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Start {
    pub(super) key: Key,
    pub(super) generation: u64,
    pub(super) watching: Watching,
}

/// Where a watch's row stood when a call read it: the generation the row
/// last ran as, or none while there is no row. Generations only grow, so a
/// row a later call set or resumed never reads the same as one read before
/// it. A row that was removed and never set again reads as none both times,
/// which can only repeat a delivery, never lose one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Revision(Option<u64>);

/// What [`WatchBook::watch`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Watched {
    /// Nothing changed: the session settled with nothing watched, or a
    /// later call decided the watch meanwhile. Whether the token watches
    /// the session.
    Unchanged { watching: bool },
    /// The session settled, so the watch it had is forgotten. Its task is
    /// the caller's to stop.
    Removed,
    /// A watch to run, in place of any task that ran it.
    Started(Start),
}

/// The watches, as the store keeps them, and what runs them.
#[derive(Debug)]
pub(super) struct WatchBook {
    file: WatchFile,
    max_watches: u32,
    /// The generation of the task running each watch: a watch replaced
    /// meanwhile runs as a newer one, and an older task's end and records
    /// are ignored.
    running: HashMap<Key, u64>,
    generations: u64,
    /// Each row's [`Revision`]: the generation of the call that last set
    /// it. Removing a row drops it.
    revisions: HashMap<Key, u64>,
    /// Slots held for watches of sessions the token names, by token and
    /// session, as many as calls hold.
    pending: HashMap<Key, u32>,
    /// Slots held for watches of sessions `new` has yet to make, by token.
    pending_new: HashMap<String, u32>,
    /// How many snapshots were taken.
    snapshots: u64,
}

impl WatchBook {
    /// The watches in `file`, for a server whose tokens may each watch
    /// `max_watches` sessions, and a start for each. Each watch follows
    /// again, which says afresh whether it stalls, so the errors it stalled
    /// with are cleared.
    pub(super) fn resume(mut file: WatchFile, max_watches: u32) -> (Self, Vec<Start>) {
        for watch in &mut file.watches {
            watch.error = None;
        }
        file.max_watches = Some(max_watches);
        let rows = file.watches.clone();
        let mut book = Self {
            file,
            max_watches,
            running: HashMap::new(),
            generations: 0,
            revisions: HashMap::new(),
            pending: HashMap::new(),
            pending_new: HashMap::new(),
            snapshots: 0,
        };
        let starts = rows.into_iter().map(|row| book.run(row)).collect();
        (book, starts)
    }

    /// Holds a slot of `token`'s budget for a watch of `session`, or of
    /// the session `new` will make (`None`); refused past the budget.
    pub(super) fn reserve(
        &mut self,
        token: &str,
        session: Option<&SessionId>,
    ) -> Result<Slot, OverBudget> {
        self.admit(token, session)?;
        match session {
            Some(session) => {
                *self
                    .pending
                    .entry((token.to_owned(), session.clone()))
                    .or_default() += 1;
            }
            None => *self.pending_new.entry(token.to_owned()).or_default() += 1,
        }
        Ok(Slot {
            token: token.to_owned(),
            session: session.cloned(),
        })
    }

    /// Gives `slot` back unused.
    pub(super) fn release(&mut self, slot: Slot) {
        match slot.session {
            Some(session) => decrement(&mut self.pending, (slot.token, session)),
            None => decrement(&mut self.pending_new, slot.token),
        }
    }

    /// Where watch `key`'s row stands, for a call to read before it
    /// awaits and hand to [`Self::watch`].
    pub(super) fn revision(&self, key: &Key) -> Revision {
        Revision(self.revisions.get(key).copied())
    }

    /// The `watch` call of `token`'s, of the session `now` is of, from
    /// where it is now; `read` is the watch's [`Revision`] read before
    /// `now`. When another call set or removed the watch since, that call
    /// came later and decided it: it is left as it is. A session that
    /// settled already is the call's own news: nothing is watched, and a
    /// watch it had is forgotten. Otherwise the news `now` is, if any, is
    /// the call's answer and is not delivered again; a watch the token does
    /// not have yet takes a slot of its budget.
    pub(super) fn watch(
        &mut self,
        token: &str,
        webhook: String,
        now: &WaitResult,
        read: Revision,
    ) -> Result<Watched, OverBudget> {
        let key = (token.to_owned(), now.session.clone());
        if self.revision(&key) != read {
            // A stalled row runs no task, so it is not watching.
            return Ok(Watched::Unchanged {
                watching: self.running.contains_key(&key),
            });
        }
        let reported = match News::of(now) {
            News::Settled(_) if self.remove(&key) => return Ok(Watched::Removed),
            News::Settled(_) => return Ok(Watched::Unchanged { watching: false }),
            News::Asks(about) => Some(about),
            News::Nothing => None,
        };
        self.admit(token, Some(&now.session))?;
        Ok(Watched::Started(self.upsert(WatchRecord {
            session: key.1,
            token: key.0,
            webhook,
            reported,
            error: None,
        })))
    }

    /// The watch of `notify`, with `slot`, of `session`, whose turn the
    /// call just started: from the turn's start.
    pub(super) fn notify(&mut self, slot: Slot, session: SessionId, webhook: String) -> Start {
        let token = slot.token.clone();
        self.release(slot);
        self.upsert(WatchRecord {
            session,
            token,
            webhook,
            reported: None,
            error: None,
        })
    }

    /// Forgets the watch `key`; whether there was one. Its task is the
    /// caller's to stop.
    pub(super) fn remove(&mut self, key: &Key) -> bool {
        self.running.remove(key);
        self.revisions.remove(key);
        let before = self.file.watches.len();
        self.file
            .watches
            .retain(|watch| (&watch.token, &watch.session) != (&key.0, &key.1));
        self.file.watches.len() != before
    }

    /// Applies how the task `generation` of watch `key` ended; whether it
    /// still ran the watch, so anything changed.
    pub(super) fn apply_end(&mut self, key: &Key, generation: u64, end: &End) -> bool {
        if !self.runs(key, generation) {
            return false;
        }
        self.running.remove(key);
        match end {
            End::Delivered | End::Dropped(_) => {
                self.remove(key);
            }
            End::Stalled(error) => {
                if let Some(watch) = self.row_mut(key) {
                    watch.error = Some(error.clone());
                }
            }
        }
        true
    }

    /// Applies what the task `generation` of watch `key` recorded, at Unix
    /// time `at`; whether it still ran the watch, so anything changed.
    pub(super) fn apply_record(
        &mut self,
        key: &Key,
        generation: u64,
        record: Record<'_>,
        at: u64,
    ) -> bool {
        if !self.runs(key, generation) {
            return false;
        }
        let Some(watch) = self.row_mut(key) else {
            return false;
        };
        let result = match record {
            Record::Delivered { reported, status } => {
                watch.reported = Some(reported.clone());
                DeliveryResult::Delivered(status)
            }
            Record::Failed { error } => DeliveryResult::Failed(error.to_owned()),
        };
        let webhook = watch.webhook.clone();
        self.file.deliveries.insert(
            webhook,
            DeliveryRecord {
                at,
                session: key.1.clone(),
                result,
            },
        );
        true
    }

    /// The watches as they are now, to save.
    pub(super) fn snapshot(&mut self) -> Snapshot {
        self.snapshots += 1;
        Snapshot {
            seq: self.snapshots,
            file: self.file.clone(),
        }
    }

    /// Checks `token`'s budget for a watch of `session`, or of the session
    /// `new` will make (`None`). A session the token watches, or holds a
    /// slot for, already takes no other: a renewal replaces its watch.
    fn admit(&self, token: &str, session: Option<&SessionId>) -> Result<(), OverBudget> {
        let sessions = self.sessions_of(token);
        if session.is_some_and(|session| sessions.contains(session)) {
            return Ok(());
        }
        let held = u32::try_from(sessions.len())
            .unwrap_or(u32::MAX)
            .saturating_add(self.pending_new.get(token).copied().unwrap_or(0));
        let asked = held.saturating_add(1);
        if asked > self.max_watches {
            return Err(OverBudget {
                holder: Holder::Token(token.to_owned()),
                limit: self.max_watches,
                asked,
            });
        }
        Ok(())
    }

    /// Saves `row` in place of the watch it replaces, and runs it, as a
    /// new revision.
    fn upsert(&mut self, row: WatchRecord) -> Start {
        match self.row_mut(&(row.token.clone(), row.session.clone())) {
            Some(watch) => *watch = row.clone(),
            None => self.file.watches.push(row.clone()),
        }
        self.run(row)
    }

    /// Runs the watch `row` as a new generation, which is also its new
    /// revision.
    fn run(&mut self, row: WatchRecord) -> Start {
        self.generations += 1;
        let key = (row.token, row.session.clone());
        self.running.insert(key.clone(), self.generations);
        self.revisions.insert(key.clone(), self.generations);
        Start {
            key,
            generation: self.generations,
            watching: Watching {
                session: row.session,
                webhook: row.webhook,
                reported: row.reported,
            },
        }
    }

    /// Whether task `generation` still runs watch `key`.
    fn runs(&self, key: &Key, generation: u64) -> bool {
        self.running.get(key) == Some(&generation)
    }

    fn row_mut(&mut self, key: &Key) -> Option<&mut WatchRecord> {
        self.file
            .watches
            .iter_mut()
            .find(|watch| (&watch.token, &watch.session) == (&key.0, &key.1))
    }

    /// The sessions `token`'s budget counts: those it watches, and those it
    /// holds slots for.
    fn sessions_of(&self, token: &str) -> HashSet<&SessionId> {
        let watched = self
            .file
            .watches
            .iter()
            .filter(|watch| watch.token == token)
            .map(|watch| &watch.session);
        let pending = self
            .pending
            .keys()
            .filter(|(holder, _)| holder == token)
            .map(|(_, session)| session);
        watched.chain(pending).collect()
    }
}

/// Takes one from `key`'s count, dropping it at zero.
fn decrement<K: Eq + std::hash::Hash>(counts: &mut HashMap<K, u32>, key: K) {
    if let Some(count) = counts.get_mut(&key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&key);
        }
    }
}

#[cfg(test)]
#[path = "book_tests.rs"]
mod tests;
