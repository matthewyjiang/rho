use pretty_assertions::assert_eq;
use rho_protocol as wire;
use rho_sdk::SessionId;

use super::session;
use crate::app::pull_requests::{
    checkout::Checkout,
    gh::ReadError,
    model::{BranchId, Budget, ChangeKey, Found, PrId, PrState, PrSummary, RepoId, Rollup},
    poller::BUDGET_FLOOR,
    record::LinkOrigin,
    service::{BranchRead, DefaultBranch, Snapshot, State, Trouble},
};

/// When each case's view is built, in unix milliseconds.
const NOW: u64 = 1_700_000_000_000;

/// When a paused host's budget resets, in unix seconds.
const RESETS_AT: i64 = 1_700_003_600;

/// When the case's PR was linked, in unix seconds.
const LINKED_AT: u64 = 1_699_990_000;

fn listed() -> SessionId {
    SessionId::from_string("listed").unwrap()
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

/// PR `number`'s summary, last updated at `updated_at`.
fn summary(number: u32, updated_at: &str) -> PrSummary {
    PrSummary {
        id: pr(number),
        title: "Widgets".into(),
        url: pr(number).url(),
        state: PrState::Open,
        change: ChangeKey {
            updated_at: updated_at.into(),
            head_oid: "1a2b3c4d5e6f".into(),
            rollup: Rollup::Passing,
        },
    }
}

/// [`summary`] as the wire sends it.
fn sent(number: u32, updated_at: &str) -> wire::PullRequestSummary {
    wire::PullRequestSummary::new(
        wire::PullRequestId::new("github.com", "acme", "widgets", number),
        "Widgets",
        format!("https://github.com/acme/widgets/pull/{number}"),
        wire::PullRequestState::Open,
        wire::PullRequestRollup::Passing,
        "1a2b3c4d5e6f",
        updated_at,
    )
}

/// PR 12 linked to the session by its user, read as `snapshot`.
fn link_12(state: &mut State, snapshot: Option<Snapshot>) {
    state
        .entry(&listed())
        .record
        .link(pr(12), pr(12).url(), LinkOrigin::User, LINKED_AT);
    if let Some(snapshot) = snapshot {
        state.snapshots.insert(pr(12), snapshot);
    }
}

/// PR 12 as the view lists it, with `summary` and `trouble`.
fn linked_12(
    summary: Option<wire::PullRequestSummary>,
    trouble: Option<wire::PullRequestTrouble>,
) -> wire::LinkedPullRequest {
    let mut linked = wire::LinkedPullRequest::new(
        wire::PullRequestId::new("github.com", "acme", "widgets", 12),
        "https://github.com/acme/widgets/pull/12",
        wire::PullRequestLinkOrigin::User,
        LINKED_AT,
    );
    linked.summary = summary;
    linked.trouble = trouble;
    linked
}

/// A client listed the session, whose workspace has `checkout`.
fn list(state: &mut State, checkout: Checkout) {
    state.interest.list(&listed(), checkout, NOW);
}

/// The branch the session has checked out, as the view says it.
fn on_branch(
    name: Option<&str>,
    lookup: wire::BranchLookup,
    trouble: Option<wire::PullRequestTrouble>,
) -> Option<wire::BranchPullRequest> {
    Some(wire::BranchPullRequest::new(
        name.map(str::to_owned),
        lookup,
        trouble,
    ))
}

/// How the case's state was built.
type Setup = fn(&mut State);

// Covers: a session's view says why there is no pull request to show: not
// looked up yet, a lookup or read that failed (keeping what an earlier one
// found), the default branch, a checkout on no branch or no forge; that a
// found PR shows its newest read; and that a paused host wins over a
// read's own trouble. Otherwise a client says "no pull request" where the
// truth is "still looking" or "could not read".
// Owner: pull request wire conversion.
#[test]
fn a_view_says_what_was_found_and_why_not() {
    let cases: [(&str, Setup, bool, wire::SessionPullRequests); 10] = [
        (
            "a link not read yet, its session never listed",
            |state| link_12(state, None),
            false,
            wire::SessionPullRequests::new(None, vec![linked_12(None, None)]),
        ),
        (
            "a link keeps its last read, and says why the next failed",
            |state| {
                let snapshot = Snapshot {
                    summary: Some(summary(12, "2026-11-22T00:00:00Z")),
                    trouble: Some(Trouble::Failed(ReadError::SignedOut("HTTP 401".into()))),
                    ..Snapshot::default()
                };
                link_12(state, Some(snapshot));
            },
            false,
            wire::SessionPullRequests::new(
                None,
                vec![linked_12(
                    Some(sent(12, "2026-11-22T00:00:00Z")),
                    Some(wire::PullRequestTrouble::SignedOut {
                        host: "github.com".into(),
                    }),
                )],
            ),
        ),
        (
            "a paused host wins over a read's own trouble",
            |state| {
                let snapshot = Snapshot {
                    trouble: Some(Trouble::Refused("no such pull request".into())),
                    ..Snapshot::default()
                };
                link_12(state, Some(snapshot));
            },
            true,
            wire::SessionPullRequests::new(
                None,
                vec![linked_12(
                    None,
                    Some(wire::PullRequestTrouble::Paused {
                        host: "github.com".into(),
                        remaining: 10,
                        floor: BUDGET_FLOOR,
                        resets_at: RESETS_AT,
                    }),
                )],
            ),
        ),
        (
            "a branch never looked up is looking",
            |state| list(state, Checkout::Branch(branch("feature"))),
            false,
            wire::SessionPullRequests::new(
                on_branch(Some("feature"), wire::BranchLookup::Looking, None),
                Vec::new(),
            ),
        ),
        (
            "a lookup that failed is still looking, and says why",
            |state| {
                list(state, Checkout::Branch(branch("feature")));
                let read = BranchRead {
                    found: None,
                    asked_at: Some(NOW),
                    trouble: Some(Trouble::Failed(ReadError::NotInstalled("no gh".into()))),
                };
                state.branches.insert(branch("feature"), read);
            },
            false,
            wire::SessionPullRequests::new(
                on_branch(
                    Some("feature"),
                    wire::BranchLookup::Looking,
                    Some(wire::PullRequestTrouble::NotInstalled),
                ),
                Vec::new(),
            ),
        ),
        (
            "an earlier answer stands while a later lookup fails",
            |state| {
                list(state, Checkout::Branch(branch("feature")));
                let read = BranchRead {
                    found: Some(Found::None),
                    asked_at: Some(NOW),
                    trouble: Some(Trouble::Failed(ReadError::Failed("timed out".into()))),
                };
                state.branches.insert(branch("feature"), read);
            },
            false,
            wire::SessionPullRequests::new(
                on_branch(
                    Some("feature"),
                    wire::BranchLookup::None,
                    Some(wire::PullRequestTrouble::Failed {
                        message: "timed out".into(),
                    }),
                ),
                Vec::new(),
            ),
        ),
        (
            "a found pull request shows its newest read",
            |state| {
                list(state, Checkout::Branch(branch("feature")));
                let read = BranchRead {
                    found: Some(Found::Pr(summary(7, "2026-11-22T00:00:00Z"))),
                    asked_at: Some(NOW),
                    trouble: None,
                };
                state.branches.insert(branch("feature"), read);
                let snapshot = Snapshot {
                    summary: Some(summary(7, "2026-11-22T00:05:00Z")),
                    ..Snapshot::default()
                };
                state.snapshots.insert(pr(7), snapshot);
            },
            false,
            wire::SessionPullRequests::new(
                on_branch(
                    Some("feature"),
                    wire::BranchLookup::Found {
                        pull_request: Box::new(sent(7, "2026-11-22T00:05:00Z")),
                    },
                    None,
                ),
                Vec::new(),
            ),
        ),
        (
            "the default branch is not looked up",
            |state| {
                list(state, Checkout::Branch(branch("main")));
                let main = DefaultBranch {
                    name: Some("main".into()),
                    at: NOW,
                };
                state.default_branches.insert(repo(), main);
            },
            false,
            wire::SessionPullRequests::new(
                on_branch(Some("main"), wire::BranchLookup::DefaultBranch, None),
                Vec::new(),
            ),
        ),
        (
            "a detached checkout names its commit",
            |state| {
                let commit = "1a2b3c4".to_owned();
                list(state, Checkout::Detached { commit });
            },
            // Nothing is read for it, so no host's pause is its trouble.
            true,
            wire::SessionPullRequests::new(
                on_branch(
                    None,
                    wire::BranchLookup::Detached {
                        commit: "1a2b3c4".into(),
                    },
                    None,
                ),
                Vec::new(),
            ),
        ),
        (
            "a branch on no forge is not looked up",
            |state| {
                let branch = "feature".to_owned();
                list(state, Checkout::NoForge { branch });
            },
            false,
            wire::SessionPullRequests::new(
                on_branch(Some("feature"), wire::BranchLookup::NoForge, None),
                Vec::new(),
            ),
        ),
    ];
    for (name, setup, paused, expected) in cases {
        let mut state = State::default();
        setup(&mut state);
        let budget = Budget {
            remaining: 10,
            resets_at: RESETS_AT,
            cost: None,
        };
        let pause = |_: &str| paused.then(|| budget.clone());
        assert_eq!(session(&state, pause, &listed()), expected, "{name}");
    }
}
