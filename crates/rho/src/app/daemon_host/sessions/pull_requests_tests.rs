use std::{path::Path, process::Command, sync::Arc};

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
    pull_requests::ScriptedForge,
};

/// When each test's requests and polls start, in unix milliseconds.
const NOW: u64 = 1_700_000_000_000;

/// Sessions in a daemon whose rho home is `home`, reading `forge`.
fn daemon_sessions(home: &TempDir, forge: &Arc<ScriptedForge>) -> Arc<Sessions> {
    Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
        forge.clone(),
    )
}

/// The pull requests `session`'s feed sent its clients, oldest first.
fn sent(session: &LoadedSession) -> Vec<wire::SessionPullRequests> {
    session
        .feed
        .buffered_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::PullRequests { pull_requests } => Some(*pull_requests),
            _ => None,
        })
        .collect()
}

/// [`TEST_PULL_REQUEST`] as the session lists it: linked by its user at
/// [`NOW`], not watched, with checks rolled up as `rollup` at its
/// `updated_at`, once read.
fn linked(read: Option<(wire::PullRequestRollup, &str)>) -> wire::LinkedPullRequest {
    let id = wire::PullRequestId::new("github.com", "acme", "widgets", 12);
    let url = "https://github.com/acme/widgets/pull/12";
    let origin = wire::PullRequestLinkOrigin::User;
    let mut linked = wire::LinkedPullRequest::new(id.clone(), url, origin, NOW / 1000);
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
    let sessions = daemon_sessions(&home, &forge);
    let session = load(&sessions, &root, Load::Saved).await;
    let id = session_to_wire(session.id.clone());
    forge.set_open(TEST_PULL_REQUEST, &["ci", "lint"], &[]);
    let after = |seconds: u64| NOW + seconds * 1000;

    let link = wire::SessionPullRequestLinkParams::new(id.clone(), TEST_PULL_REQUEST);
    sessions.pull_request_link(link, NOW).await.unwrap();
    let target = PullRequestTarget::Linked {
        pull_request: TEST_PULL_REQUEST.into(),
    };
    let mut show = wire::SessionPullRequestShowParams::new(id, target);
    show.read = PullRequestRead::Fresh;
    sessions.pull_request_show(show, NOW).await.unwrap();
    forge.take_asked();

    let reporter = sessions.report_pull_requests(|| NOW);
    tokio::pin!(reporter);
    // Its first pass finds nothing new, then waits for a change.
    assert!(futures_util::poll!(reporter.as_mut()).is_pending());
    let service = sessions.pull_requests.service();
    // Due again for the show's lease, its checks still running, unchanged.
    service.tick(after(20)).await;
    let quiet_reads = forge.take_asked().len();
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
        (quiet_reads, sent(&session)),
        (
            1,
            vec![
                wire::SessionPullRequests::new(None, vec![linked(None)]),
                wire::SessionPullRequests::new(not_git.clone(), vec![linked(Some(pending))]),
                wire::SessionPullRequests::new(not_git, vec![linked(Some(failing))]),
            ]
        )
    );
}

/// Runs `git args` in `dir`, isolated from the caller's repository and
/// config.
fn git(dir: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    for name in crate::git::LOCATION_ENV {
        command.env_remove(name);
    }
    let output = command
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=rho", "-c", "user.email=rho@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// What a case's session works in.
enum Workspace {
    /// A plain directory.
    Plain,
    /// A repository with its head detached, whose `origin` is acme/widgets
    /// on github.com.
    DetachedOnGithub,
}

// Covers: a client shows a linked pull request by the text that linked it,
// resolved the same way in the session's workspace, a detached head's
// included: `#N` names a PR in the origin's repository whatever is checked
// out. Otherwise a panel that just linked `#12` cannot show it.
// Owner: daemon session pull requests.
#[tokio::test]
async fn a_linked_pull_request_shows_by_the_text_that_linked_it() {
    let cases = [
        (
            "a plain workspace, by owner/name#N",
            Workspace::Plain,
            TEST_PULL_REQUEST,
        ),
        (
            "a detached head whose origin is on github.com, by #N",
            Workspace::DetachedOnGithub,
            "#12",
        ),
    ];
    for (name, workspace, text) in cases {
        let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        match workspace {
            Workspace::Plain => {}
            Workspace::DetachedOnGithub => {
                git(root.path(), &["init", "--quiet", "--initial-branch=main"]);
                git(
                    root.path(),
                    &["commit", "--quiet", "--allow-empty", "-m", "base"],
                );
                git(root.path(), &["checkout", "--quiet", "--detach"]);
                let origin = "git@github.com:acme/widgets.git";
                git(root.path(), &["remote", "add", "origin", origin]);
            }
        }
        let forge = ScriptedForge::new();
        forge.set_open(TEST_PULL_REQUEST, &[], &[]);
        let sessions = daemon_sessions(&home, &forge);
        let session = load(&sessions, &root, Load::Saved).await;
        let id = session_to_wire(session.id.clone());

        let link = wire::SessionPullRequestLinkParams::new(id.clone(), text);
        let linked = sessions.pull_request_link(link, NOW).await.unwrap();
        let target = PullRequestTarget::Linked {
            pull_request: text.into(),
        };
        let show = wire::SessionPullRequestShowParams::new(id, target);
        let shown = sessions.pull_request_show(show, NOW).await;

        let pr = wire::PullRequestId::new("github.com", "acme", "widgets", 12);
        assert_eq!(
            (
                linked.pull_request,
                shown.map(|shown| shown.detail.map(|detail| detail.summary.id)),
            ),
            (pr.clone(), Ok(Some(pr))),
            "{name}"
        );
    }
}
