//! What a tick reads at `now`, from the state alone: each watched pull
//! request at [`super::poller::WATCH_POLL`], and what clients list or show
//! at their leases' cadences ([`super::interest`]), each at the fastest
//! cadence anything wants it at, and only once that cadence is due.
//!
//! A listed pull request gets its summary only. A watched or shown one gets
//! its detail when [`detail_due`] says so, else its summary, and its detail
//! in the tick's second query when the summary moved.
//!
//! Also what a client's show reads itself before it answers
//! ([`show_reads`]).

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};

use super::{
    interest::{Cadence, Shown, SHOWN_POLL},
    model::{BranchId, FetchPlan, Found, PrId, PrSummary, Rollup},
    poller::WATCH_POLL,
    service::{ShowRead, Snapshot, State},
    state::Baseline,
    watch,
};

/// How old a watched or shown PR's last full read may get before a tick
/// reads it in full again although its summary did not change.
///
/// Receipt: t3's `QUIET_REREAD_MS` (`PullRequestWatchReactor.ts:53`). The
/// summary's change key misses a base-branch push that makes the PR
/// conflict, and an edit to branch protection; this bounds how late such a
/// wake can be. 6 details an hour per quiet watch; 25 watches are 150
/// details, about 60 points an hour.
pub(crate) const QUIET_REREAD: Duration = Duration::from_secs(10 * 60);

/// What one tick asks one host.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Due {
    /// Branch lookups, summaries and details, each sorted.
    pub(super) plan: FetchPlan,
    /// The watched and shown PRs due: those whose summary this tick reads
    /// get their detail in a second query when it moved. A listed PR never
    /// gets a detail.
    pub(super) wants_detail: HashSet<PrId>,
}

/// Why a tick may read one pull request.
#[derive(Default)]
struct Want {
    /// The fastest cadence anything wants it at.
    cadence: Option<Cadence>,
    /// Whether it needs a summary of its own: it is linked. A PR a branch
    /// lookup found comes with that lookup.
    linked: bool,
    /// Whether a watch of it has not taken its baseline.
    pending: bool,
}

impl Want {
    fn at(&mut self, cadence: Cadence) {
        self.cadence = Some(self.cadence.map_or(cadence, |fastest| fastest.min(cadence)));
    }
}

/// What each host is asked at `now` (unix milliseconds), by host; a host
/// with nothing due is not there.
pub(super) fn due(state: &State, now: u64) -> BTreeMap<String, Due> {
    let mut prs: HashMap<&PrId, Want> = HashMap::new();
    for (session, entry) in &state.sessions {
        for link in entry.record.links() {
            let leased = state
                .interest
                .cadence(session, &Shown::Linked(link.pr.clone()), now);
            // Whether its watch is still to take its baseline.
            let watched = link
                .watch
                .as_ref()
                .map(|watch| watch.baseline == Baseline::Pending);
            if leased.is_none() && watched.is_none() {
                continue;
            }
            let want = prs.entry(&link.pr).or_default();
            want.linked = true;
            if let Some(cadence) = leased {
                want.at(cadence);
            }
            if let Some(pending) = watched {
                want.at(Cadence::Watched);
                want.pending |= pending;
            }
        }
    }
    let mut branches: HashMap<&BranchId, Cadence> = HashMap::new();
    for (session, interest) in state.interest.sessions() {
        let Some(branch) = interest.checkout().branch() else {
            continue;
        };
        let Some(cadence) = state.interest.cadence(session, &Shown::Branch, now) else {
            continue;
        };
        if state.is_default(branch) {
            continue;
        }
        let fastest = branches.entry(branch).or_insert(cadence);
        *fastest = (*fastest).min(cadence);
        // The PR its last lookup found is read at its cadence too.
        if let Some(pr) = state.found(branch) {
            prs.entry(pr).or_default().at(cadence);
        }
    }

    let mut hosts: BTreeMap<String, Due> = BTreeMap::new();
    for (branch, cadence) in branches {
        let asked_at = state.branches.get(branch).and_then(|read| read.asked_at);
        if cadence.due(asked_at, now) {
            let due = hosts.entry(branch.repo.host.clone()).or_default();
            due.plan.branches.push(branch.clone());
        }
    }
    for (pr, want) in prs {
        let Some(cadence) = want.cadence else {
            continue;
        };
        let snapshot = state.snapshots.get(pr);
        if !cadence.due(snapshot.and_then(|snapshot| snapshot.asked_at), now) {
            continue;
        }
        let due = hosts.entry(pr.repo.host.clone()).or_default();
        match cadence {
            Cadence::Shown | Cadence::Watched => {
                due.wants_detail.insert(pr.clone());
                if want.pending || detail_due(snapshot, now) {
                    due.plan.details.push(pr.clone());
                } else if want.linked {
                    due.plan.links.push(pr.clone());
                }
            }
            Cadence::Listed => {
                if want.linked {
                    due.plan.links.push(pr.clone());
                }
            }
        }
    }
    hosts.retain(|_, due| !due.plan.is_empty());
    for due in hosts.values_mut() {
        let FetchPlan {
            branches,
            links,
            details,
        } = &mut due.plan;
        branches.sort_by(|a, b| {
            (&a.repo.owner, &a.repo.name, &a.branch).cmp(&(&b.repo.owner, &b.repo.name, &b.branch))
        });
        links.sort_by(by_number);
        details.sort_by(by_number);
    }
    hosts
}

/// How long the poll loop sleeps between ticks at `now`: [`SHOWN_POLL`]
/// while a client shows anything, else [`WATCH_POLL`].
pub(super) fn tick_period(state: &State, now: u64) -> Duration {
    if state.interest.shows_any(now) {
        SHOWN_POLL
    } else {
        WATCH_POLL
    }
}

/// The pull requests a client shows at `now`: those shown as linked, and
/// those their branch's last lookup found for sessions shown by branch.
pub(super) fn shown(state: &State, now: u64) -> HashSet<PrId> {
    let mut shown = HashSet::new();
    for (_, interest) in state.interest.sessions() {
        for target in interest.shown(now) {
            match target {
                Shown::Linked(pr) => {
                    shown.insert(pr.clone());
                }
                Shown::Branch => {
                    let found = interest
                        .checkout()
                        .branch()
                        .and_then(|branch| state.found(branch));
                    shown.extend(found.cloned());
                }
            }
        }
    }
    shown
}

/// What a client's show reads before it answers, as [`show_reads`] judges
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShowOf<'a> {
    /// A pull request linked to the session.
    Linked(&'a PrId),
    /// The pull request a lookup of the session's branch finds.
    Branch(&'a BranchId),
}

/// What `state` keeps of a show's target.
enum Kept {
    /// Something to answer with: the PR's detail, or a lookup that found
    /// no PR, or the default branch, which has none.
    Answer,
    /// Nothing to answer with, and no attempt to read it failed.
    Missing,
    /// Nothing to answer with, since the attempt at this time (unix
    /// milliseconds) failed.
    FailedAt(u64),
}

/// Whether a client's show of `of` reads the forge at `now` (unix
/// milliseconds) before it answers: always when `read` is fresh; else when
/// `state` keeps nothing to answer with (a summary a tick read is not a
/// detail), unless the last attempt failed within the shown cadence, so a
/// failing target is not read on every show.
pub(super) fn show_reads(state: &State, of: ShowOf<'_>, read: ShowRead, now: u64) -> bool {
    let kept = match of {
        ShowOf::Linked(pr) => kept_detail(state.snapshots.get(pr)),
        ShowOf::Branch(branch) => kept_lookup(state, branch),
    };
    match (read, kept) {
        (ShowRead::Fresh, _) | (ShowRead::Cached, Kept::Missing) => true,
        (ShowRead::Cached, Kept::Answer) => false,
        (ShowRead::Cached, Kept::FailedAt(at)) => Cadence::Shown.due(Some(at), now),
    }
}

/// What `snapshot` keeps to answer a show of its PR with.
fn kept_detail(snapshot: Option<&Snapshot>) -> Kept {
    let Some(snapshot) = snapshot else {
        return Kept::Missing;
    };
    match (&snapshot.detail, &snapshot.trouble, snapshot.asked_at) {
        (Some(_), _, _) => Kept::Answer,
        (None, Some(_), Some(at)) => Kept::FailedAt(at),
        (None, Some(_), None) | (None, None, _) => Kept::Missing,
    }
}

/// What `state` keeps to answer a show of `branch`'s pull request with.
fn kept_lookup(state: &State, branch: &BranchId) -> Kept {
    if state.is_default(branch) {
        return Kept::Answer;
    }
    let Some(read) = state.branches.get(branch) else {
        return Kept::Missing;
    };
    match (&read.found, &read.trouble, read.asked_at) {
        (Some(Found::Pr(summary)), _, _) => kept_detail(state.snapshots.get(&summary.id)),
        (Some(Found::None | Found::OnlyForks(_)), _, _) => Kept::Answer,
        (None, Some(_), Some(at)) => Kept::FailedAt(at),
        (None, Some(_), None) | (None, None, _) => Kept::Missing,
    }
}

/// Whether a watched or shown PR needs its detail at `now` whatever its
/// summary says: it has none yet, its checks were running at the last
/// read, or its last detail is [`QUIET_REREAD`] old.
///
/// Running checks come from the last detail: GitHub's rollup reads failing
/// as soon as one check fails, while others still run, and checks past
/// those the detail lists may be running too. A pending rollup in a newer
/// summary counts as well.
pub(super) fn detail_due(snapshot: Option<&Snapshot>, now: u64) -> bool {
    let Some(snapshot) = snapshot else {
        return true;
    };
    let Some(detailed) = &snapshot.detail else {
        return true;
    };
    let detail = &detailed.detail;
    let running = detail.unread_checks() > 0
        || detail
            .checks
            .iter()
            .any(|check| watch::running(check.status))
        || snapshot
            .summary
            .as_ref()
            .is_some_and(|summary| summary.change.rollup == Rollup::Pending);
    running || Duration::from_millis(now.saturating_sub(detailed.at)) >= QUIET_REREAD
}

/// Whether `summary` differs from the PR's last detail, so its detail
/// would read differently now.
pub(super) fn moved_since_detail(snapshot: Option<&Snapshot>, summary: &PrSummary) -> bool {
    snapshot
        .and_then(|snapshot| snapshot.detail.as_ref())
        .is_none_or(|detailed| {
            let last = &detailed.detail.summary;
            last.change != summary.change || last.state != summary.state
        })
}

/// Orders PRs by repository, then number.
pub(super) fn by_number(a: &PrId, b: &PrId) -> Ordering {
    (&a.repo.owner, &a.repo.name, a.number).cmp(&(&b.repo.owner, &b.repo.name, b.number))
}

#[cfg(test)]
#[path = "due_tests.rs"]
mod tests;
