//! The PR service's types as `rho-protocol` sends them: one session's view
//! ([`session`]), and each pull request's summary and detail. The model
//! stays the service's own, so the wire, the records and the parser change
//! apart, as `daemon_host/wire.rs` keeps the host's types.

use rho_protocol as wire;
use rho_sdk::SessionId;

use super::{
    checkout::Checkout,
    due,
    gh::ReadError,
    model::{
        Budget, ChangeKey, CheckRun, CheckStatus, Found, MergeMethod, Mergeable, Post, PrDetail,
        PrId, PrState, PrSummary, Review, Rollup, Thread, ThreadState, Verdict,
    },
    poller::BUDGET_FLOOR,
    record::{Link, LinkOrigin},
    service::{Detailed, State, Trouble},
};

/// `session`'s pull requests in `state`, as clients see them: its links in
/// the order linked, each with its newest summary, and its branch's, once
/// a client listed or showed it since the daemon started. `paused` names a
/// host's budget while reads of it wait; that wins over a read's own
/// trouble, since nothing is read there until it resets.
pub(super) fn session(
    state: &State,
    paused: impl Fn(&str) -> Option<Budget>,
    session: &SessionId,
) -> wire::SessionPullRequests {
    let linked = state.sessions.get(session).map_or_else(Vec::new, |entry| {
        entry
            .record
            .links()
            .iter()
            .map(|link| linked(state, &paused, link))
            .collect()
    });
    let branch = state
        .interest
        .session(session)
        .and_then(|interest| interest.checkout())
        .map(|checkout| branch(state, &paused, checkout));
    wire::SessionPullRequests::new(branch, linked)
}

/// `link` with the last read of its pull request.
fn linked(
    state: &State,
    paused: &impl Fn(&str) -> Option<Budget>,
    link: &Link,
) -> wire::LinkedPullRequest {
    let snapshot = state.snapshots.get(&link.pr);
    let host = link.pr.repo.host.as_str();
    let mut linked = wire::LinkedPullRequest::new(
        wire::PullRequestId::from(&link.pr),
        link.url.clone(),
        link.origin.into(),
        link.linked_at,
    );
    linked.watched = link.watch.is_some();
    linked.summary = snapshot
        .and_then(|snapshot| snapshot.summary.as_ref())
        .map(wire::PullRequestSummary::from);
    linked.trouble = trouble(
        host,
        paused(host),
        snapshot.and_then(|snapshot| snapshot.trouble.as_ref()),
    );
    linked
}

/// What `checkout` is on, and what the last lookup of its branch found.
/// A branch never answered is still looking; a lookup that failed keeps
/// what the one before it found, and says why, as does a failed read of
/// the pull request it found.
fn branch(
    state: &State,
    paused: &impl Fn(&str) -> Option<Budget>,
    checkout: &Checkout,
) -> wire::BranchPullRequest {
    let branch = match checkout {
        Checkout::NotGit => {
            return wire::BranchPullRequest::new(None, wire::BranchLookup::NotGit, None)
        }
        Checkout::Detached { commit } => {
            let lookup = wire::BranchLookup::Detached {
                commit: commit.clone(),
            };
            return wire::BranchPullRequest::new(None, lookup, None);
        }
        Checkout::NoForge { branch } => {
            let lookup = wire::BranchLookup::NoForge;
            return wire::BranchPullRequest::new(Some(branch.clone()), lookup, None);
        }
        Checkout::Branch(branch) => branch,
    };
    let name = Some(branch.branch.clone());
    if due::is_default(state, branch) {
        return wire::BranchPullRequest::new(name, wire::BranchLookup::DefaultBranch, None);
    }
    let read = state.branches.get(branch);
    let found = read.and_then(|read| read.found.as_ref());
    let lookup = match found {
        None => wire::BranchLookup::Looking,
        Some(Found::None) => wire::BranchLookup::None,
        Some(Found::OnlyForks(forks)) => wire::BranchLookup::OnlyForks {
            read: u32::try_from(*forks).unwrap_or(u32::MAX),
        },
        Some(Found::Pr(found)) => {
            // A read of the PR since the lookup is newer; the lookup's own
            // summary went into its snapshot too.
            let newest = state
                .snapshots
                .get(&found.id)
                .and_then(|snapshot| snapshot.summary.as_ref())
                .unwrap_or(found);
            wire::BranchLookup::Found {
                pull_request: Box::new(newest.into()),
            }
        }
    };
    let found_trouble = state
        .found(branch)
        .and_then(|pr| state.snapshots.get(pr))
        .and_then(|snapshot| snapshot.trouble.as_ref());
    let own = read
        .and_then(|read| read.trouble.as_ref())
        .or(found_trouble);
    let host = branch.repo.host.as_str();
    wire::BranchPullRequest::new(name, lookup, trouble(host, paused(host), own))
}

/// Why what was last read of something on `host` may be out of date: its
/// budget while `paused`, else why its last read failed.
fn trouble(
    host: &str,
    paused: Option<Budget>,
    failed: Option<&Trouble>,
) -> Option<wire::PullRequestTrouble> {
    if let Some(budget) = paused {
        return Some(wire::PullRequestTrouble::Paused {
            host: host.to_owned(),
            remaining: budget.remaining,
            floor: BUDGET_FLOOR,
            resets_at: budget.resets_at,
        });
    }
    Some(match failed? {
        Trouble::Refused(why) => wire::PullRequestTrouble::NotFound {
            message: why.clone(),
        },
        Trouble::Failed(ReadError::NotInstalled(_)) => wire::PullRequestTrouble::NotInstalled,
        Trouble::Failed(ReadError::SignedOut(_)) => wire::PullRequestTrouble::SignedOut {
            host: host.to_owned(),
        },
        Trouble::Failed(ReadError::Failed(why)) => wire::PullRequestTrouble::Failed {
            message: why.clone(),
        },
    })
}

impl From<&PrId> for wire::PullRequestId {
    fn from(pr: &PrId) -> Self {
        Self::new(&pr.repo.host, &pr.repo.owner, &pr.repo.name, pr.number)
    }
}

impl From<&PrSummary> for wire::PullRequestSummary {
    fn from(summary: &PrSummary) -> Self {
        let PrSummary {
            id,
            title,
            url,
            state,
            change:
                ChangeKey {
                    updated_at,
                    head_oid,
                    rollup,
                },
        } = summary;
        Self::new(
            id.into(),
            title,
            url,
            (*state).into(),
            (*rollup).into(),
            head_oid,
            updated_at,
        )
    }
}

impl From<&Detailed> for wire::PullRequestDetail {
    /// Every field of the detail, named, so a new one is not forgotten.
    fn from(detailed: &Detailed) -> Self {
        let Detailed { detail, at } = detailed;
        let PrDetail {
            summary,
            author,
            head,
            base,
            body,
            additions,
            deletions,
            changed_files,
            review,
            checks,
            checks_total,
            posts,
            older_posts,
            threads,
            older_threads,
            mergeable,
            merge_methods,
            viewer,
            merged_by,
            merge_commit,
        } = detail;
        let mut sent = Self::new(summary.into(), at / 1000);
        sent.author.clone_from(author);
        sent.head.clone_from(head);
        sent.base.clone_from(base);
        sent.body.clone_from(body);
        sent.additions = *additions;
        sent.deletions = *deletions;
        sent.changed_files = *changed_files;
        sent.review = (*review).into();
        sent.mergeable = (*mergeable).into();
        sent.merge_methods = merge_methods.iter().map(|&method| method.into()).collect();
        sent.checks = checks.iter().map(Into::into).collect();
        sent.checks_total = *checks_total;
        sent.posts = posts.iter().map(Into::into).collect();
        sent.older_posts = *older_posts;
        sent.threads = threads.iter().map(Into::into).collect();
        sent.older_threads = *older_threads;
        sent.viewer.clone_from(viewer);
        sent.merged_by.clone_from(merged_by);
        sent.merge_commit.clone_from(merge_commit);
        sent
    }
}

impl From<&CheckRun> for wire::PullRequestCheck {
    fn from(check: &CheckRun) -> Self {
        let CheckRun {
            name,
            workflow,
            status,
            url,
            required,
        } = check;
        Self::new(
            name,
            workflow.clone(),
            (*status).into(),
            url.clone(),
            *required,
        )
    }
}

impl From<&Post> for wire::PullRequestPost {
    fn from(post: &Post) -> Self {
        let Post {
            id,
            author,
            author_association,
            at,
            body,
            verdict,
            url,
        } = post;
        let mut sent = Self::new(id, author, *at, body, url);
        sent.author_association.clone_from(author_association);
        sent.verdict = (*verdict).map(Into::into);
        sent
    }
}

impl From<&Thread> for wire::PullRequestThread {
    fn from(thread: &Thread) -> Self {
        let Thread {
            id,
            path,
            line,
            state,
            comments,
            more_comments,
        } = thread;
        Self::new(
            id,
            path,
            *line,
            (*state).into(),
            comments.iter().map(Into::into).collect(),
            *more_comments,
        )
    }
}

impl From<LinkOrigin> for wire::PullRequestLinkOrigin {
    fn from(origin: LinkOrigin) -> Self {
        match origin {
            LinkOrigin::User => Self::User,
            LinkOrigin::Agent => Self::Agent,
        }
    }
}

impl From<PrState> for wire::PullRequestState {
    fn from(state: PrState) -> Self {
        match state {
            PrState::Open => Self::Open,
            PrState::Draft => Self::Draft,
            PrState::Merged => Self::Merged,
            PrState::Closed => Self::Closed,
        }
    }
}

impl From<Rollup> for wire::PullRequestRollup {
    fn from(rollup: Rollup) -> Self {
        match rollup {
            Rollup::None => Self::None,
            Rollup::Pending => Self::Pending,
            Rollup::Passing => Self::Passing,
            Rollup::Failing => Self::Failing,
        }
    }
}

impl From<Review> for wire::PullRequestReview {
    fn from(review: Review) -> Self {
        match review {
            Review::None => Self::None,
            Review::Required => Self::Required,
            Review::ChangesRequested => Self::ChangesRequested,
            Review::Approved => Self::Approved,
        }
    }
}

impl From<Mergeable> for wire::PullRequestMergeable {
    /// The model's `Unknown` is "not worked out yet"; the wire keeps
    /// `Unknown` for a value a newer daemon sends.
    fn from(mergeable: Mergeable) -> Self {
        match mergeable {
            Mergeable::Yes => Self::Clean,
            Mergeable::Conflicting => Self::Conflicting,
            Mergeable::Unknown => Self::Undetermined,
        }
    }
}

impl From<MergeMethod> for wire::PullRequestMergeMethod {
    fn from(method: MergeMethod) -> Self {
        match method {
            MergeMethod::Merge => Self::Merge,
            MergeMethod::Squash => Self::Squash,
            MergeMethod::Rebase => Self::Rebase,
        }
    }
}

impl From<CheckStatus> for wire::PullRequestCheckStatus {
    fn from(status: CheckStatus) -> Self {
        match status {
            CheckStatus::Failed => Self::Failed,
            CheckStatus::Running => Self::Running,
            CheckStatus::Pending => Self::Pending,
            CheckStatus::Cancelled => Self::Cancelled,
            CheckStatus::Passed => Self::Passed,
            CheckStatus::Neutral => Self::Neutral,
            CheckStatus::Skipped => Self::Skipped,
        }
    }
}

impl From<Verdict> for wire::PullRequestVerdict {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Approved => Self::Approved,
            Verdict::ChangesRequested => Self::ChangesRequested,
            Verdict::Commented => Self::Commented,
            Verdict::Dismissed => Self::Dismissed,
        }
    }
}

impl From<ThreadState> for wire::PullRequestThreadState {
    fn from(state: ThreadState) -> Self {
        match state {
            ThreadState::Open => Self::Open,
            ThreadState::Outdated => Self::Outdated,
            ThreadState::Resolved => Self::Resolved,
        }
    }
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
