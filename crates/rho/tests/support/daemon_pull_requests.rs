//! A session's pull requests over the daemon's socket, through the real
//! binary: a client links one, shows it, lists it, and unlinks it, and an
//! attached client hears each change. The daemon reads GitHub with the `gh`
//! on its own `PATH`, here a script that answers with a captured read.

use std::{os::unix::fs::PermissionsExt, path::Path};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{
        SessionPullRequestLink, SessionPullRequestList, SessionPullRequestShow,
        SessionPullRequestUnlink,
    },
    BranchLookup, BranchPullRequest, ListedSessionPullRequests, PullRequestId, PullRequestTarget,
    SessionEvent, SessionPullRequestLinkParams, SessionPullRequestListParams,
    SessionPullRequestListResponse, SessionPullRequestShowParams, SessionPullRequestUnlinkParams,
    SessionPullRequests,
};

use super::sessions::{Follower, Sandbox};

/// The pull request the captured read is of.
const PULL_REQUEST: &str = "cli/cli#14578";

/// A detail read of [`PULL_REQUEST`] as `gh api graphql` printed it, its
/// one repository aliased `r0` and the pull request `p0`: the query the
/// daemon builds to read one pull request's detail.
const CAPTURED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/app/pull_requests/fixtures/gh_conversation.json"
);

/// Writes an executable `gh` into `bin` that answers with [`CAPTURED`]
/// under the alias it is asked: `p0` for a detail read, else `l0`, which
/// a summary read of the one linked pull request asks and which the same
/// object answers, a detail holding every field of a summary. So a poll
/// that reads the link between the requests below, as one 15 s after the
/// show's read may, reads it as the show did, however slow the run.
fn fake_gh(bin: &Path) {
    std::fs::create_dir_all(bin).unwrap();
    let gh = bin.join("gh");
    let script = format!(
        "#!/bin/sh\n\
         case \"$*\" in\n\
         *'p0: pullRequest'*) cat '{CAPTURED}' ;;\n\
         *) sed 's/\"p0\":/\"l0\":/' '{CAPTURED}' ;;\n\
         esac\n"
    );
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Whether an event carries the session's pull requests, as `done` wants
/// them.
fn carrying(done: impl Fn(&SessionPullRequests) -> bool) -> impl Fn(&SessionEvent) -> bool {
    move |event| matches!(event, SessionEvent::PullRequests { pull_requests } if done(pull_requests))
}

// Covers: the daemon serves a client's link, show, list and unlink of a
// session's pull request: the link is saved under the rho home, the show
// reads the pull request through `gh` before it answers, the list carries
// that read and the workspace's checkout, the unlink deletes the record,
// and an attached client hears each change as a `pull_requests` event.
// Owner: CLI process (daemon protocol).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_links_shows_lists_and_unlinks_a_pull_request() {
    let sandbox = Sandbox::new();
    let bin = sandbox.isolated.rho_home().join("fake-gh-bin");
    fake_gh(&bin);
    let mut command = sandbox.rho("fixed");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    command.env("PATH", path);
    let (_daemon, client) = sandbox.spawn_daemon(command, &[]).await;
    let session = sandbox.new_session(&client).await;
    let (mut follower, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    let record = sandbox
        .isolated
        .rho_home()
        .join("daemon")
        .join("pull-requests")
        .join(format!("{}.json", session.as_str()));
    let list = async || {
        let params = SessionPullRequestListParams::new(vec![session.clone()]);
        client
            .request::<SessionPullRequestList>(&params)
            .await
            .unwrap()
    };

    let linked = client
        .request::<SessionPullRequestLink>(&SessionPullRequestLinkParams::new(
            session.clone(),
            PULL_REQUEST,
        ))
        .await
        .unwrap();
    follower
        .read_until(carrying(|set| set.linked.len() == 1))
        .await;
    let saved = record.exists();

    let target = PullRequestTarget::Linked {
        pull_request: PULL_REQUEST.into(),
    };
    let shown = client
        .request::<SessionPullRequestShow>(&SessionPullRequestShowParams::new(
            session.clone(),
            target,
        ))
        .await
        .unwrap();
    follower
        .read_until(carrying(|set| {
            set.linked.iter().any(|link| link.summary.is_some())
        }))
        .await;
    let listed = list().await;

    let unlinked = client
        .request::<SessionPullRequestUnlink>(&SessionPullRequestUnlinkParams::new(
            session.clone(),
            PULL_REQUEST,
        ))
        .await
        .unwrap();
    follower
        .read_until(carrying(|set| set.linked.is_empty()))
        .await;
    let listed_after = list().await;

    let id = PullRequestId::new("github.com", "cli", "cli", 14578);
    let detail = shown.detail.expect("the show read the pull request");
    let not_git = Some(BranchPullRequest::new(None, BranchLookup::NotGit, None));
    let [mut link] = <[_; 1]>::try_from(linked.pull_requests.linked.clone()).unwrap();
    let read_before = link.summary.clone();
    link.summary = Some(detail.summary.clone());
    let listed_as = |links| {
        SessionPullRequestListResponse::new(vec![ListedSessionPullRequests::new(
            session.clone(),
            SessionPullRequests::new(not_git.clone(), links),
        )])
    };
    assert_eq!(
        (
            (linked.pull_request, read_before, saved),
            detail.summary.id.clone(),
            listed,
            (unlinked.pull_requests.linked, record.exists()),
            listed_after,
        ),
        (
            (id.clone(), None, true),
            id,
            listed_as(vec![link]),
            (Vec::new(), false),
            listed_as(Vec::new()),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
