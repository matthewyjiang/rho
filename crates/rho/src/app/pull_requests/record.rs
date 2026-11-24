//! What the daemon keeps for one session: the pull requests linked to it,
//! the watch on each it watches, and the news waiting to be delivered.
//! [`super::store`] saves one [`SessionRecord`] per session; the methods
//! here keep its invariants, so every caller changes it the same way.

use serde::{Deserialize, Serialize};

use super::{
    model::{PrId, PrState},
    state::{News, WatchState},
};

/// How many news items a session keeps queued before it drops the oldest.
///
/// Receipt: a wake's text budget is 16 KiB, as for processes and subagents
/// (`tools/process/notify.rs`, `tools/agent_output.rs`). An item is about
/// 300 bytes (a 200-char snippet, as t3's `SNIPPET_LENGTH`, plus a URL), so
/// about 50 fit in one wake; more could not be shown anyway.
pub(crate) const NEWS_PER_SESSION: usize = 50;

/// One session's linked pull requests and its undelivered news.
///
/// The session is named by the file the store keeps it in, not here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionRecord {
    /// In the order they were linked, each PR once.
    links: Vec<Link>,
    /// Oldest first.
    news: Vec<News>,
    /// News dropped by [`Self::queue_news`] that no delivered wake has
    /// told of yet.
    news_dropped: u32,
    /// The [`News::seq`] the next queued news gets: how many were ever
    /// queued, so seqs never repeat while the record lives.
    #[serde(default)]
    next_seq: u64,
}

/// A pull request linked to a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Link {
    pub(crate) pr: PrId,
    /// Where the PR shows on its forge.
    pub(crate) url: String,
    pub(crate) origin: LinkOrigin,
    /// Unix seconds.
    pub(crate) linked_at: u64,
    /// `Some` while rho watches it for the session.
    pub(crate) watch: Option<WatchState>,
    /// The head the agent last heard of: at the watch's start, and with
    /// each news since. News on another head says new commits came.
    #[serde(default)]
    pub(crate) told_head: Option<String>,
    /// How the pull request stood at the last read of it made since it was
    /// linked; `None` until one is kept. Saved, so a merge that came while
    /// the daemon was down still reads as one.
    #[serde(default)]
    pub(crate) stood: Option<Stood>,
}

/// How a linked pull request stood at a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stood {
    Open,
    /// Closed without merging.
    Closed,
    Merged,
}

impl From<PrState> for Stood {
    fn from(state: PrState) -> Self {
        match state {
            PrState::Open | PrState::Draft => Self::Open,
            PrState::Closed => Self::Closed,
            PrState::Merged => Self::Merged,
        }
    }
}

impl Link {
    /// Whether the poller reads the pull request with no one asking, to
    /// find its merge: until a read finds it ended. A closed pull request
    /// that is reopened is read again once a watch or a client reads it.
    pub(crate) fn awaits_merge(&self) -> bool {
        match self.stood {
            None | Some(Stood::Open) => true,
            Some(Stood::Closed | Stood::Merged) => false,
        }
    }
}

/// What [`SessionRecord::keep_standing`] changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StandingKept {
    /// Whether any link's standing changed, so the record needs saving.
    pub(crate) changed: bool,
    /// Whether a link that an earlier read found unmerged now reads
    /// merged. A pull request first read as merged, as one linked after it
    /// merged, is no merge found.
    pub(crate) found_merge: bool,
}

/// Who linked a pull request to a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LinkOrigin {
    /// A person, through a client.
    User,
    /// The session's agent, through a tool.
    Agent,
}

impl SessionRecord {
    /// Links `pr`, unwatched. Linking a linked PR changes nothing, so the
    /// first link's origin and time stand: whether it was new.
    pub(crate) fn link(&mut self, pr: PrId, url: String, origin: LinkOrigin, now: u64) -> bool {
        if self.links.iter().any(|link| link.pr == pr) {
            return false;
        }
        self.links.push(Link {
            pr,
            url,
            origin,
            linked_at: now,
            watch: None,
            told_head: None,
            stood: None,
        });
        true
    }

    /// Removes `pr`'s link, and with it its watch: whether it was linked.
    /// News already queued for it stays, to be delivered as it happened.
    pub(crate) fn unlink(&mut self, pr: &PrId) -> bool {
        let before = self.links.len();
        self.links.retain(|link| link.pr != *pr);
        self.links.len() != before
    }

    pub(crate) fn links(&self) -> &[Link] {
        &self.links
    }

    /// `pr`'s link, to start, advance or end its watch.
    pub(crate) fn link_mut(&mut self, pr: &PrId) -> Option<&mut Link> {
        self.links.iter_mut().find(|link| link.pr == *pr)
    }

    /// The links rho watches, each with its watch.
    pub(crate) fn watching(&self) -> impl Iterator<Item = (&Link, &WatchState)> {
        self.links
            .iter()
            .filter_map(|link| link.watch.as_ref().map(|watch| (link, watch)))
    }

    /// Keeps how each linked pull request stands, as `last_read` says the
    /// newest read of it found it, with when that read was, in unix
    /// milliseconds. A link it has no read of keeps what it had, and so
    /// does one linked in the second of that read or after it: sessions
    /// share reads, and a read older than the link may show as open a pull
    /// request that had merged by the time it was linked.
    pub(crate) fn keep_standing(
        &mut self,
        last_read: impl Fn(&PrId) -> Option<(Stood, u64)>,
    ) -> StandingKept {
        let mut kept = StandingKept::default();
        for link in &mut self.links {
            let Some((read, at)) = last_read(&link.pr) else {
                continue;
            };
            if at / 1000 <= link.linked_at || link.stood == Some(read) {
                continue;
            }
            kept.changed = true;
            kept.found_merge |= read == Stood::Merged && link.stood.is_some();
            link.stood = Some(read);
        }
        kept
    }

    /// Whether every linked pull request has merged, and one is linked:
    /// the session's work on them is over.
    pub(crate) fn all_merged(&self) -> bool {
        !self.links.is_empty()
            && self
                .links
                .iter()
                .all(|link| link.stood == Some(Stood::Merged))
    }

    /// Undelivered news, oldest first.
    pub(crate) fn news(&self) -> &[News] {
        &self.news
    }

    /// How many news items were dropped since a wake last told of drops.
    pub(crate) fn news_dropped(&self) -> u32 {
        self.news_dropped
    }

    /// How many news items were ever queued: grows with each, so a caller
    /// can tell news that came since it last looked.
    pub(crate) fn news_generation(&self) -> u64 {
        self.next_seq
    }

    /// Queues `news` as the next [`News::seq`], keeping at most `cap` items
    /// ([`NEWS_PER_SESSION`] in production) besides those `in_flight`, the
    /// seqs a turn is delivering. Past it the oldest item that is neither
    /// an end nor in flight is dropped and counted in
    /// [`Self::news_dropped`], so the next wake can say so. An end is never
    /// dropped: it is the one news of a watch that stopped, and a session
    /// has at most one per link, so ends alone stay bounded. Nor is news in
    /// flight, which may yet be delivered: the queue then holds at most
    /// `cap` more than the batch in flight.
    pub(crate) fn queue_news(&mut self, mut news: News, cap: usize, in_flight: &[u64]) {
        news.seq = self.next_seq;
        self.next_seq += 1;
        self.news.push(news);
        while self.news.len() > cap {
            let Some(oldest) = self
                .news
                .iter()
                .position(|news| !news.is_end() && !in_flight.contains(&news.seq))
            else {
                break;
            };
            self.news.remove(oldest);
            self.news_dropped = self.news_dropped.saturating_add(1);
        }
    }

    /// Removes the news whose seqs are `delivered` from the queue, after a
    /// wake carrying it was applied. A seq no longer queued is ignored.
    ///
    /// The drop count clears only when this empties the queue. News queued
    /// while the wake was in flight was not in it, and drops among that
    /// news were not told: repeating a count is harmless, losing one is not.
    pub(crate) fn acknowledge(&mut self, delivered: &[u64]) {
        self.news.retain(|queued| !delivered.contains(&queued.seq));
        if self.news.is_empty() {
            self.news_dropped = 0;
        }
    }

    /// Whether it holds nothing worth keeping: no links and no news. The
    /// store deletes an empty record's file.
    pub(crate) fn is_empty(&self) -> bool {
        self.links.is_empty() && self.news.is_empty()
    }
}
