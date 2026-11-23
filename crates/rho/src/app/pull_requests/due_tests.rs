use std::{collections::BTreeMap, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::{due, tick_period, Due};
use crate::app::pull_requests::{
    checkout::Checkout,
    interest::{Shown, SHOWN_POLL},
    model::{
        BranchId, ChangeKey, CheckRun, CheckStatus, FetchPlan, Found, Mergeable, PrDetail, PrId,
        PrState, PrSummary, RepoId, Review, Rollup,
    },
    poller::WATCH_POLL,
    record::LinkOrigin,
    service::{BranchRead, Detailed, Snapshot, State},
    state::{Baseline, WatchState},
};

/// When each case's tick runs, in unix milliseconds.
const NOW: u64 = 1_700_000_000_000;

/// `seconds` before [`NOW`].
fn ago(seconds: u64) -> u64 {
    NOW - seconds * 1000
}

fn session() -> SessionId {
    SessionId::from_string("looked-at").unwrap()
}

fn repo() -> RepoId {
    RepoId {
        host: "github.com".into(),
        owner: "acme".into(),
        name: "widgets".into(),
    }
}

fn pr(number: u32) -> PrId {
    PrId {
        repo: repo(),
        number,
    }
}

fn branch(name: &str) -> BranchId {
    BranchId {
        repo: repo(),
        branch: name.into(),
    }
}

/// PR `number`, open with its one check passed: nothing makes its detail
/// due before [`super::QUIET_REREAD`].
fn quiet(number: u32) -> PrDetail {
    PrDetail {
        summary: PrSummary {
            id: pr(number),
            title: "Widgets".into(),
            url: pr(number).url(),
            state: PrState::Open,
            change: ChangeKey {
                updated_at: "2026-11-22T00:00:00Z".into(),
                head_oid: "1a2b3c4d5e6f".into(),
                rollup: Rollup::Passing,
            },
        },
        author: "me".into(),
        head: "feature".into(),
        base: "main".into(),
        body: String::new(),
        additions: 1,
        deletions: 1,
        changed_files: 1,
        review: Review::None,
        checks: vec![CheckRun {
            name: "ci".into(),
            workflow: None,
            status: CheckStatus::Passed,
            url: None,
            required: None,
        }],
        checks_total: 1,
        posts: Vec::new(),
        older_posts: 0,
        threads: Vec::new(),
        older_threads: 0,
        mergeable: Mergeable::Yes,
        merge_methods: Vec::new(),
        viewer: Some("me".into()),
        merged_by: None,
        merge_commit: None,
    }
}

fn link(state: &mut State, number: u32) {
    let pr = pr(number);
    let url = pr.url();
    state
        .entry(&session())
        .record
        .link(pr, url, LinkOrigin::User, 0);
}

/// Links PR `number` and watches it, its baseline taken.
fn watch(state: &mut State, number: u32) {
    link(state, number);
    let entry = state.entry(&session());
    let link = entry.record.link_mut(&pr(number)).unwrap();
    link.watch = Some(WatchState {
        baseline: Baseline::Taken,
        ..WatchState::new(0)
    });
}

/// PR `number` was last asked for at `at`, and read in full then.
fn read_quiet(state: &mut State, number: u32, at: u64) {
    let detail = quiet(number);
    state.snapshots.insert(
        pr(number),
        Snapshot {
            summary: Some(detail.summary.clone()),
            detail: Some(Detailed { detail, at }),
            read_at: Some(at),
            asked_at: Some(at),
            trouble: None,
        },
    );
}

/// PR `number` was last asked for at `at`, and its summary read then.
fn read_summary(state: &mut State, number: u32, at: u64) {
    state.snapshots.insert(
        pr(number),
        Snapshot {
            summary: Some(quiet(number).summary),
            asked_at: Some(at),
            read_at: Some(at),
            ..Snapshot::default()
        },
    );
}

fn on(name: &str) -> Option<Checkout> {
    Some(Checkout::Branch(branch(name)))
}

/// What a tick asks github.com.
fn asks(branches: &[&str], links: &[u32], details: &[u32], wants_detail: &[u32]) -> Option<Due> {
    Some(Due {
        plan: FetchPlan {
            branches: branches.iter().copied().map(branch).collect(),
            links: links.iter().copied().map(pr).collect(),
            details: details.iter().copied().map(pr).collect(),
        },
        wants_detail: wants_detail.iter().copied().map(pr).collect(),
    })
}

struct Case {
    name: &'static str,
    given: fn(&mut State),
    /// What github.com is asked; `None` when nothing is due.
    asked: Option<Due>,
    period: Duration,
}

// Covers: the lease table. With no lease only watches are read; a listed
// session's links get summaries and its branch a lookup, never a detail,
// once their cadence is due; a shown PR gets its detail when it needs one,
// else its summary, and a shown branch's PR comes with its lookup; the
// default branch and a checkout on no forge are never looked up; a watch is
// due within the slack; and a lease that ran out reads nothing.
// Owner: pure unit (due.rs)
#[test]
fn a_lease_decides_what_a_tick_reads() {
    let cases = [
        Case {
            name: "linked, not watched, no lease",
            given: |state| link(state, 12),
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "linked by a listed session, asked within its period",
            given: |state| {
                link(state, 12);
                state.interest.list(&session(), None, ago(10));
                read_summary(state, 12, ago(100));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "linked by a listed session, due: its summary, though it has no detail",
            given: |state| {
                link(state, 12);
                state.interest.list(&session(), None, ago(10));
                read_summary(state, 12, ago(290));
            },
            asked: asks(&[], &[12], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "a listed session's branch, never looked up",
            given: |state| {
                state.interest.list(&session(), on("feature"), NOW);
            },
            asked: asks(&["feature"], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "a listed session on its repository's default branch",
            given: |state| {
                state.interest.list(&session(), on("main"), NOW);
                state.default_branches.insert(repo(), Some("main".into()));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "a listed session on a branch of no forge's repository",
            given: |state| {
                let checkout = Checkout::NoForge {
                    branch: "feature".into(),
                };
                state.interest.list(&session(), Some(checkout), NOW);
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "a listed lease that ran out",
            given: |state| {
                link(state, 12);
                state.interest.list(&session(), on("feature"), ago(601));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "a shown linked PR with no detail: its detail",
            given: |state| {
                link(state, 12);
                state
                    .interest
                    .show(&session(), None, Shown::Linked(pr(12)), NOW);
            },
            asked: asks(&[], &[], &[12], &[12]),
            period: SHOWN_POLL,
        },
        Case {
            name: "a shown linked PR, quiet and due: its summary",
            given: |state| {
                link(state, 12);
                state
                    .interest
                    .show(&session(), None, Shown::Linked(pr(12)), NOW);
                read_quiet(state, 12, ago(30));
            },
            asked: asks(&[], &[12], &[], &[12]),
            period: SHOWN_POLL,
        },
        Case {
            name: "a shown branch whose lookup found a quiet PR: the lookup brings it",
            given: |state| {
                state
                    .interest
                    .show(&session(), on("feature"), Shown::Branch, NOW);
                let found = BranchRead {
                    found: Some(Found::Pr(quiet(12).summary)),
                    asked_at: Some(ago(30)),
                    trouble: None,
                };
                state.branches.insert(branch("feature"), found);
                read_quiet(state, 12, ago(30));
            },
            asked: asks(&["feature"], &[], &[], &[12]),
            period: SHOWN_POLL,
        },
        Case {
            name: "watched with no lease, asked 30 s ago",
            given: |state| {
                watch(state, 12);
                read_quiet(state, 12, ago(30));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "watched with no lease, asked 45 s ago: due within the slack",
            given: |state| {
                watch(state, 12);
                read_quiet(state, 12, ago(45));
            },
            asked: asks(&[], &[12], &[], &[12]),
            period: WATCH_POLL,
        },
        Case {
            name: "watched and shown, asked 30 s ago: at the shown cadence",
            given: |state| {
                watch(state, 12);
                state
                    .interest
                    .show(&session(), None, Shown::Linked(pr(12)), NOW);
                read_quiet(state, 12, ago(30));
            },
            asked: asks(&[], &[12], &[], &[12]),
            period: SHOWN_POLL,
        },
    ];
    for Case {
        name,
        given,
        asked,
        period,
    } in cases
    {
        let mut state = State::default();
        given(&mut state);

        let expected: BTreeMap<String, Due> = asked
            .into_iter()
            .map(|due| ("github.com".to_owned(), due))
            .collect();
        assert_eq!(
            (due(&state, NOW), tick_period(&state, NOW)),
            (expected, period),
            "{name}"
        );
    }
}
