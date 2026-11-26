use std::{collections::BTreeMap, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::{due, show_reads, tick_period, Due, ShowOf, QUIET_REREAD};
use crate::app::pull_requests::{
    checkout::Checkout,
    gh::ReadError,
    interest::{Shown, SHOWN_POLL},
    model::{
        BranchId, ChangeKey, CheckRun, CheckStatus, FetchPlan, Found, Mergeable, PrDetail, PrId,
        PrState, PrSummary, RepoId, Review, Rollup,
    },
    poller::WATCH_POLL,
    record::{LinkOrigin, Stood},
    service::{BranchRead, DefaultBranch, Detailed, ShowRead, Snapshot, State, Trouble},
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
        queued_workflows: 0,
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

/// Links PR `number`, which the last read of it found as `stood`.
fn link_read(state: &mut State, number: u32, stood: Stood) {
    link(state, number);
    let entry = state.entry(&session());
    entry.record.link_mut(&pr(number)).unwrap().stood = Some(stood);
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
            unjudged: None,
            read_at: Some(at),
            asked_at: Some(at),
            show_failed_at: None,
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

/// A read at `at` saw `name` as the repository's default branch.
fn default_branch(state: &mut State, name: &str, at: u64) {
    let observed = DefaultBranch {
        name: Some(name.into()),
        at,
    };
    state.default_branches.insert(repo(), observed);
}

fn on(name: &str) -> Checkout {
    Checkout::Branch(branch(name))
}

/// What a tick asks github.com: `branches` looked up, of which those also
/// in `shown` at the shown cadence.
fn asks(
    branches: &[&str],
    links: &[u32],
    details: &[u32],
    wants_detail: &[u32],
    shown: &[&str],
) -> Option<Due> {
    Some(Due {
        plan: FetchPlan {
            branches: branches.iter().copied().map(branch).collect(),
            links: links.iter().copied().map(pr).collect(),
            details: details.iter().copied().map(pr).collect(),
        },
        wants_detail: wants_detail.iter().copied().map(pr).collect(),
        shown_branches: shown.iter().copied().map(branch).collect(),
    })
}

struct Case {
    name: &'static str,
    given: fn(&mut State),
    /// What github.com is asked; `None` when nothing is due.
    asked: Option<Due>,
    period: Duration,
}

// Covers: the lease table. With no lease, watches are read, and so is a
// link's summary, at its own slower cadence, until a read found its pull
// request merged or closed, so a merge nobody asks about is found; a
// listed session's links get summaries and its branch a lookup, never a
// detail, once their cadence is due; a shown PR gets its detail when it needs one,
// else its summary, and a shown branch's PR comes with its lookup; the
// default branch is not looked up while its observation is younger than the
// reread period (else a default that moved is never seen), and a checkout on
// no forge never is; a watch is due within the slack; and a lease that ran
// out reads nothing.
// Owner: pure unit (due.rs)
#[test]
fn a_lease_decides_what_a_tick_reads() {
    let cases = [
        Case {
            name: "linked, not watched, no lease, never read: its summary",
            given: |state| link(state, 12),
            asked: asks(&[], &[12], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "linked, no lease, read open within the linked period",
            given: |state| {
                link_read(state, 12, Stood::Open);
                read_summary(state, 12, ago(100));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "linked, no lease, read open a linked period ago: its summary",
            given: |state| {
                link_read(state, 12, Stood::Open);
                read_summary(state, 12, ago(290));
            },
            asked: asks(&[], &[12], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "linked since its pull request was last asked for, as for another session",
            given: |state| {
                let (pr, url) = (pr(12), pr(12).url());
                let record = &mut state.entry(&session()).record;
                record.link(pr, url, LinkOrigin::Agent, ago(50) / 1000);
                read_summary(state, 12, ago(100));
            },
            asked: asks(&[], &[12], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "linked, no lease, one due: the host's other open links with it",
            given: |state| {
                link_read(state, 12, Stood::Open);
                link_read(state, 13, Stood::Open);
                link_read(state, 14, Stood::Merged);
                read_summary(state, 12, ago(290));
                read_summary(state, 13, ago(100));
                read_summary(state, 14, ago(600));
            },
            asked: asks(&[], &[12, 13], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "linked, no lease, read merged or closed: not read again",
            given: |state| {
                link_read(state, 12, Stood::Merged);
                link_read(state, 13, Stood::Closed);
                read_summary(state, 12, ago(600));
                read_summary(state, 13, ago(600));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "linked by a listed session, asked within its period",
            given: |state| {
                link(state, 12);
                state.interest.list(&session(), Checkout::NotGit, ago(10));
                read_summary(state, 12, ago(100));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "linked by a listed session, due: its summary, though it has no detail",
            given: |state| {
                link(state, 12);
                state.interest.list(&session(), Checkout::NotGit, ago(10));
                read_summary(state, 12, ago(290));
            },
            asked: asks(&[], &[12], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "a listed session's branch, never looked up",
            given: |state| {
                state.interest.list(&session(), on("feature"), NOW);
            },
            asked: asks(&["feature"], &[], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "a listed session on its repository's default branch",
            given: |state| {
                state.interest.list(&session(), on("main"), NOW);
                default_branch(state, "main", ago(10));
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "a listed session on a default branch observed the reread period ago: looked up",
            given: |state| {
                state.interest.list(&session(), on("main"), NOW);
                let observed = NOW - u64::try_from(QUIET_REREAD.as_millis()).unwrap();
                default_branch(state, "main", observed);
            },
            asked: asks(&["main"], &[], &[], &[], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "a listed session on a branch of no forge's repository",
            given: |state| {
                let checkout = Checkout::NoForge {
                    branch: "feature".into(),
                };
                state.interest.list(&session(), checkout, NOW);
            },
            asked: None,
            period: WATCH_POLL,
        },
        Case {
            name: "a listed lease that ran out",
            given: |state| {
                link_read(state, 12, Stood::Closed);
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
                    .show(&session(), Checkout::NotGit, Shown::Linked(pr(12)), NOW);
            },
            asked: asks(&[], &[], &[12], &[12], &[]),
            period: SHOWN_POLL,
        },
        Case {
            name: "a shown linked PR, quiet and due: its summary",
            given: |state| {
                link(state, 12);
                state
                    .interest
                    .show(&session(), Checkout::NotGit, Shown::Linked(pr(12)), NOW);
                read_quiet(state, 12, ago(30));
            },
            asked: asks(&[], &[12], &[], &[12], &[]),
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
            asked: asks(&["feature"], &[], &[], &[12], &["feature"]),
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
            asked: asks(&[], &[12], &[], &[12], &[]),
            period: WATCH_POLL,
        },
        Case {
            name: "watched and shown, asked 30 s ago: at the shown cadence",
            given: |state| {
                watch(state, 12);
                state
                    .interest
                    .show(&session(), Checkout::NotGit, Shown::Linked(pr(12)), NOW);
                read_quiet(state, 12, ago(30));
            },
            asked: asks(&[], &[12], &[], &[12], &[]),
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

/// PR `number` was last asked for at `at`, and that read failed; nothing
/// was read of it before.
fn read_failed(state: &mut State, number: u32, at: u64) {
    state.snapshots.insert(
        pr(number),
        Snapshot {
            asked_at: Some(at),
            trouble: Some(Trouble::Failed(ReadError::Failed("timed out".into()))),
            ..Snapshot::default()
        },
    );
}

/// A read of PR `number` outside a tick failed at `at`, and nothing was
/// read of it before; a tick last asked for it at `asked`, if ever.
fn show_failed(state: &mut State, number: u32, asked: Option<u64>, at: u64) {
    state.snapshots.insert(
        pr(number),
        Snapshot {
            asked_at: asked,
            show_failed_at: Some(at),
            trouble: Some(Trouble::Failed(ReadError::Failed("timed out".into()))),
            ..Snapshot::default()
        },
    );
}

/// PR `number` as [`quiet`], after an edit that moved its change key.
fn edited(number: u32) -> PrDetail {
    let mut detail = quiet(number);
    detail.summary.change.updated_at = "2026-11-22T00:05:00Z".into();
    detail
}

/// A tick read PR `number` in full 30 s ago, and its summary since moved.
fn moved_since_read(state: &mut State, number: u32) {
    read_quiet(state, number, ago(30));
    let snapshot = state.snapshots.get_mut(&pr(number)).unwrap();
    snapshot.summary = Some(edited(number).summary);
}

/// Branch `feature` was last looked up at `at`, and found `found`.
fn looked_up(state: &mut State, found: Found, at: u64) {
    let read = BranchRead {
        found: Some(found),
        asked_at: Some(at),
        trouble: None,
    };
    state.branches.insert(branch("feature"), read);
}

/// What a case's client shows.
#[derive(Clone, Copy)]
enum Of {
    /// PR 12, linked.
    Linked,
    /// The session's branch `feature`.
    Feature,
}

/// A show case: its name, the state before it, what it shows, how, and
/// whether it reads first.
type ShowCase = (&'static str, fn(&mut State), Of, ShowRead, bool);

// Covers: a cached show reads first only when nothing kept answers it: a
// PR never read, or read only as a summary (as a listed tick reads it),
// and a branch never looked up or whose PR has no detail; never when the
// detail is there or the lookup found no PR, nor on the default branch;
// and after a failed attempt, a tick's or a show's own (a watched PR's
// show never moves the tick's ask), only once the shown cadence is due
// again; a detail older than the summary is no answer, unless a show read
// a newer one. A fresh show always reads. Otherwise a panel opened after
// the sidebar's tick answers with no detail or one older than its summary,
// or a failing target is read on every show.
// Owner: pure unit (due.rs)
#[test]
fn a_show_reads_only_what_nothing_kept_answers() {
    let cases: [ShowCase; 15] = [
        (
            "a linked PR never read",
            |_| {},
            Of::Linked,
            ShowRead::Cached,
            true,
        ),
        (
            "a linked PR a listed tick read as a summary 5 s ago",
            |state| read_summary(state, 12, ago(5)),
            Of::Linked,
            ShowRead::Cached,
            true,
        ),
        (
            "a linked PR read in full 5 s ago",
            |state| read_quiet(state, 12, ago(5)),
            Of::Linked,
            ShowRead::Cached,
            false,
        ),
        (
            "a linked PR read in full 5 s ago, shown fresh",
            |state| read_quiet(state, 12, ago(5)),
            Of::Linked,
            ShowRead::Fresh,
            true,
        ),
        (
            "a linked PR whose read failed 5 s ago",
            |state| read_failed(state, 12, ago(5)),
            Of::Linked,
            ShowRead::Cached,
            false,
        ),
        (
            "a linked PR whose read failed 15 s ago: due within the slack",
            |state| read_failed(state, 12, ago(15)),
            Of::Linked,
            ShowRead::Cached,
            true,
        ),
        (
            "a linked PR no tick asked for, whose show's read failed 5 s ago",
            |state| show_failed(state, 12, None, ago(5)),
            Of::Linked,
            ShowRead::Cached,
            false,
        ),
        (
            "a linked PR a tick asked for 20 s ago, whose show's read failed 5 s ago",
            |state| show_failed(state, 12, Some(ago(20)), ago(5)),
            Of::Linked,
            ShowRead::Cached,
            false,
        ),
        (
            "a linked PR read in full, its summary since moved",
            |state| moved_since_read(state, 12),
            Of::Linked,
            ShowRead::Cached,
            true,
        ),
        (
            "a linked PR read in full, its summary since moved, a show read it since",
            |state| {
                moved_since_read(state, 12);
                let snapshot = state.snapshots.get_mut(&pr(12)).unwrap();
                snapshot.unjudged = Some(Detailed {
                    detail: edited(12),
                    at: ago(5),
                });
            },
            Of::Linked,
            ShowRead::Cached,
            false,
        ),
        (
            "a branch never looked up",
            |_| {},
            Of::Feature,
            ShowRead::Cached,
            true,
        ),
        (
            "a branch whose lookup found no PR",
            |state| looked_up(state, Found::None, ago(5)),
            Of::Feature,
            ShowRead::Cached,
            false,
        ),
        (
            "a branch whose lookup found a PR it read only as a summary",
            |state| {
                looked_up(state, Found::Pr(quiet(12).summary), ago(5));
                read_summary(state, 12, ago(5));
            },
            Of::Feature,
            ShowRead::Cached,
            true,
        ),
        (
            "a branch whose PR was read in full",
            |state| {
                looked_up(state, Found::Pr(quiet(12).summary), ago(5));
                read_quiet(state, 12, ago(5));
            },
            Of::Feature,
            ShowRead::Cached,
            false,
        ),
        (
            "the default branch, never looked up",
            |state| default_branch(state, "feature", ago(5)),
            Of::Feature,
            ShowRead::Cached,
            false,
        ),
    ];
    let (pr_12, feature) = (pr(12), branch("feature"));
    for (name, given, of, read, expected) in cases {
        let mut state = State::default();
        given(&mut state);
        let of = match of {
            Of::Linked => ShowOf::Linked(&pr_12),
            Of::Feature => ShowOf::Branch(&feature),
        };

        assert_eq!(show_reads(&state, of, read, NOW), expected, "{name}");
    }
}
