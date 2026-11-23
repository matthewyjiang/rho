//! What clients look at: a lease on each session a client lists, and on
//! each target it shows. A lease is a time it lasts until, compared with
//! `now`; nothing sweeps them, and a client never releases one. It only
//! adds reads: with no lease, the poller reads watched pull requests and
//! nothing else, as before clients could ask.
//!
//! In memory only, and keyed by session with no client identity: two
//! clients on one session share its leases. An entry outlives its leases,
//! keeping the checkout last resolved, until its session is forgotten.

use std::{
    collections::{hash_map, HashMap},
    time::Duration,
};

use rho_sdk::SessionId;

use super::{checkout::Checkout, model::PrId, poller::WATCH_POLL};

/// How often a pull request on a client's screen is read.
///
/// Receipt: rho-ui's own `POLL` (rho-ui `pull_requests.rs:32-35`), one
/// point a poll: 120 points an hour of GitHub's 5,000.
pub(crate) const SHOWN_POLL: Duration = Duration::from_secs(30);

/// How often the pull requests of a session a client lists are read.
///
/// Receipt: rho-ui's own `INDEX_POLL` (rho-ui `pull_requests.rs:37-41`),
/// 12 points an hour per host, every listed session in one query.
pub(crate) const LISTED_POLL: Duration = Duration::from_secs(300);

/// A lease lasts this many periods of its cadence. Chosen: one late or
/// lost renewal does not stop the reads, and a client that left costs at
/// most two more.
const LEASE_PERIODS: u32 = 2;

/// A target is due this long before its period ends, so a read that took a
/// few seconds does not push it to the next tick. Chosen: half the fastest
/// tick ([`SHOWN_POLL`]); the slowest read measured took 4.9 s
/// (`gh_query::DETAILS_PER_QUERY`). A watched PR is then read every 45 to
/// 75 s rather than every 60.
pub(super) const DUE_SLACK: Duration = Duration::from_secs(15);

/// How often a target is read, fastest first: the fastest of a target's
/// cadences is its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Cadence {
    /// On a client's screen: [`SHOWN_POLL`].
    Shown,
    /// Watched for a session's agent: [`WATCH_POLL`].
    Watched,
    /// In a client's list of sessions: [`LISTED_POLL`].
    Listed,
}

impl Cadence {
    pub(super) fn period(self) -> Duration {
        match self {
            Self::Shown => SHOWN_POLL,
            Self::Watched => WATCH_POLL,
            Self::Listed => LISTED_POLL,
        }
    }

    /// Whether a target last asked at `asked_at` is due at `now` (both unix
    /// milliseconds): never asked, or asked a period ago, less
    /// [`DUE_SLACK`].
    pub(super) fn due(self, asked_at: Option<u64>, now: u64) -> bool {
        asked_at.is_none_or(|at| {
            now.saturating_add(millis(DUE_SLACK)) >= at.saturating_add(millis(self.period()))
        })
    }
}

/// What a client shows of one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Shown {
    /// The pull request of the branch its workspace has checked out.
    Branch,
    /// A pull request linked to it.
    Linked(PrId),
}

/// Whether a lease was taken anew. A renewal changes nothing the poller
/// reads, so only a new lease wakes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lease {
    /// No live lease of its kind was there, or the checkout changed.
    New,
    Renewed,
}

/// Every session a client has listed or shown since the daemon started.
#[derive(Debug, Default)]
pub(super) struct Interest {
    sessions: HashMap<SessionId, SessionInterest>,
}

/// What clients look at of one session.
#[derive(Debug)]
pub(super) struct SessionInterest {
    /// Until when a client lists the session, in unix milliseconds; 0 for
    /// never.
    listed_until: u64,
    /// Each target on a client's screen, and until when, in unix
    /// milliseconds; those run out are dropped when the next is shown.
    shown: Vec<(Shown, u64)>,
    /// What its workspace had checked out when a client last listed or
    /// showed it.
    checkout: Checkout,
}

impl SessionInterest {
    /// What its workspace had checked out at the last list or show.
    pub(super) fn checkout(&self) -> &Checkout {
        &self.checkout
    }

    /// The targets a client shows at `now`.
    pub(super) fn shown(&self, now: u64) -> impl Iterator<Item = &Shown> + '_ {
        self.shown
            .iter()
            .filter(move |(_, until)| *until > now)
            .map(|(shown, _)| shown)
    }
}

impl Interest {
    /// A client lists `session`, whose workspace has `checkout`, at `now`
    /// (unix milliseconds).
    pub(super) fn list(&mut self, session: &SessionId, checkout: Checkout, now: u64) -> Lease {
        let (entry, moved) = self.keep(session, checkout);
        let live = entry.listed_until > now;
        entry.listed_until = now.saturating_add(lease(LISTED_POLL));
        if live && !moved {
            Lease::Renewed
        } else {
            Lease::New
        }
    }

    /// A client shows `what` of `session`, whose workspace has `checkout`,
    /// at `now` (unix milliseconds).
    pub(super) fn show(
        &mut self,
        session: &SessionId,
        checkout: Checkout,
        what: Shown,
        now: u64,
    ) -> Lease {
        let (entry, moved) = self.keep(session, checkout);
        let lasts_until = now.saturating_add(lease(SHOWN_POLL));
        entry.shown.retain(|(_, until)| *until > now);
        let live = match entry.shown.iter().position(|(shown, _)| *shown == what) {
            Some(ix) => {
                entry.shown[ix].1 = lasts_until;
                true
            }
            None => {
                entry.shown.push((what, lasts_until));
                false
            }
        };
        if live && !moved {
            Lease::Renewed
        } else {
            Lease::New
        }
    }

    /// The fastest cadence a live lease on `session` gives `target` at
    /// `now`: shown when a client shows it, else listed when a client lists
    /// the session.
    pub(super) fn cadence(&self, session: &SessionId, target: &Shown, now: u64) -> Option<Cadence> {
        let entry = self.sessions.get(session)?;
        if entry.shown(now).any(|shown| shown == target) {
            return Some(Cadence::Shown);
        }
        (entry.listed_until > now).then_some(Cadence::Listed)
    }

    /// Whether any lease on `session` lives at `now`.
    pub(super) fn leased(&self, session: &SessionId, now: u64) -> bool {
        self.sessions
            .get(session)
            .is_some_and(|entry| entry.listed_until > now || entry.shown(now).next().is_some())
    }

    /// Whether a client shows anything at `now`.
    pub(super) fn shows_any(&self, now: u64) -> bool {
        self.sessions
            .values()
            .any(|entry| entry.shown(now).next().is_some())
    }

    /// `session`'s entry; `None` when no client listed or showed it since
    /// the daemon started, or since it was forgotten.
    pub(super) fn session(&self, session: &SessionId) -> Option<&SessionInterest> {
        self.sessions.get(session)
    }

    pub(super) fn sessions(&self) -> impl Iterator<Item = (&SessionId, &SessionInterest)> + '_ {
        self.sessions.iter()
    }

    /// `session` was deleted.
    pub(super) fn forget(&mut self, session: &SessionId) {
        self.sessions.remove(session);
    }

    /// `session`'s entry, made with no lease when it has none, keeping
    /// `checkout`; and whether that differs from the checkout it kept.
    fn keep(&mut self, session: &SessionId, checkout: Checkout) -> (&mut SessionInterest, bool) {
        match self.sessions.entry(session.clone()) {
            hash_map::Entry::Occupied(entry) => {
                let entry = entry.into_mut();
                let moved = entry.checkout != checkout;
                entry.checkout = checkout;
                (entry, moved)
            }
            hash_map::Entry::Vacant(entry) => {
                let entry = entry.insert(SessionInterest {
                    listed_until: 0,
                    shown: Vec::new(),
                    checkout,
                });
                (entry, true)
            }
        }
    }
}

/// How long a lease at a cadence of `period` lasts, in milliseconds.
fn lease(period: Duration) -> u64 {
    millis(period).saturating_mul(u64::from(LEASE_PERIODS))
}

pub(super) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
