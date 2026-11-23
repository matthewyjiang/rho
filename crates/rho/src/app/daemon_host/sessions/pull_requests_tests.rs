use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, PullRequestRead, PullRequestTarget};
use tempfile::TempDir;

use crate::app::{
    daemon_host::{
        jobs::{JobRegistry, JobStore},
        sessions::{
            pull_request_wake::TEST_PULL_REQUEST,
            test_sessions::{load, Load},
            LoadedSession, Sessions,
        },
        wire::session_to_wire,
    },
    pull_requests::{unix_ms, ScriptedForge},
};

/// The pull requests `session`'s feed sent its clients, oldest first, each
/// with its links' times zeroed: they are the wall clock's.
fn sent(session: &LoadedSession) -> Vec<wire::SessionPullRequests> {
    session
        .feed
        .buffered_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::PullRequests { pull_requests } => Some(*pull_requests),
            _ => None,
        })
        .map(|mut pull_requests| {
            for linked in &mut pull_requests.linked {
                linked.linked_at = 0;
            }
            pull_requests
        })
        .collect()
}

/// [`TEST_PULL_REQUEST`] as the session lists it: linked by its user, not
/// watched, with checks rolled up as `rollup` at its `updated_at`, once
/// read.
fn linked(read: Option<(wire::PullRequestRollup, &str)>) -> wire::LinkedPullRequest {
    let id = wire::PullRequestId::new("github.com", "acme", "widgets", 12);
    let url = "https://github.com/acme/widgets/pull/12";
    let mut linked =
        wire::LinkedPullRequest::new(id.clone(), url, wire::PullRequestLinkOrigin::User, 0);
    linked.summary = read.map(|(rollup, updated_at)| {
        let state = wire::PullRequestState::Open;
        wire::PullRequestSummary::new(
            id,
            "Widgets",
            url,
            state,
            rollup,
            "1a2b3c4d5e6f",
            updated_at,
        )
    });
    linked
}

// Covers: a link and a show each tell the session's attached clients its
// pull requests once, before they answer; a poll that reads the same
// pull request again tells them nothing; one that reads a failed check
// tells them once, through the reporter. Otherwise a client misses the
// change, or is sent the set on every poll until its 4,096-event backlog
// fills and it is detached as lagged.
// Owner: daemon session pull requests.
#[tokio::test]
async fn a_change_reaches_an_attached_client_once() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let forge = ScriptedForge::new();
    let sessions = Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
        forge.clone(),
    );
    let session = load(&sessions, &root, Load::Saved).await;
    let id = session_to_wire(session.id.clone());
    forge.set_open(TEST_PULL_REQUEST, &["ci", "lint"], &[]);
    // The handlers read the wall clock; the polls below run as if this
    // many seconds after it.
    let started = unix_ms();
    let after = |seconds: u64| started + seconds * 1000;

    let link = wire::SessionPullRequestLinkParams::new(id.clone(), TEST_PULL_REQUEST);
    sessions.pull_request_link(link).await.unwrap();
    let target = PullRequestTarget::Linked {
        pull_request: TEST_PULL_REQUEST.into(),
    };
    let mut show = wire::SessionPullRequestShowParams::new(id, target);
    show.read = PullRequestRead::Fresh;
    sessions.pull_request_show(show).await.unwrap();

    let reporter = sessions.report_pull_requests();
    tokio::pin!(reporter);
    // Its first pass finds nothing new, then waits for a change.
    assert!(futures_util::poll!(reporter.as_mut()).is_pending());
    let service = sessions.pull_requests.service();
    // Due again for the show's lease, its checks still running, unchanged.
    service.tick(after(20)).await;
    assert!(futures_util::poll!(reporter.as_mut()).is_pending());
    forge.set_open(TEST_PULL_REQUEST, &["lint"], &["ci"]);
    service.tick(after(40)).await;
    assert!(futures_util::poll!(reporter.as_mut()).is_pending());

    let not_git = Some(wire::BranchPullRequest::new(
        None,
        wire::BranchLookup::NotGit,
        None,
    ));
    let pending = (wire::PullRequestRollup::Pending, "2026-11-22T00:00:00Z");
    let failing = (wire::PullRequestRollup::Failing, "2026-11-22T00:01:00Z");
    assert_eq!(
        sent(&session),
        vec![
            wire::SessionPullRequests::new(None, vec![linked(None)]),
            wire::SessionPullRequests::new(not_git.clone(), vec![linked(Some(pending))]),
            wire::SessionPullRequests::new(not_git, vec![linked(Some(failing))]),
        ]
    );
}
