//! The watch evaluator: what changed on a watched pull request since the
//! last read, and whether the watch ends. Pure: the poller passes in what it
//! read and keeps [`Report::next`] for the next read.
//!
//! Each rule is one function over the next state:
//! - [`checks`]: newly failed checks, the gate passing, and the one wake
//!   owed once running checks finish; a new head starts them over.
//! - [`remarks`]: comments and reviews by others since the last one told.
//! - [`conflict`]: the branch starting to conflict with its base.
//! - [`ended`] and [`comment_loop`]: a merge or close, and a bot reply loop.
//!
//! A watch's first read is its baseline: the agent saw those checks and
//! conflicts when it began the watch, so they are recorded and not told.

use super::{
    gh::ReadError,
    model::{CheckRun, CheckStatus, Mergeable, Post, PrDetail, PrState},
    state::{Baseline, Change, CheckRef, End, Failure, Gate, Owed, Remark, WatchState},
};

/// Comment-only reports in a row that end a watch as [`End::CommentLoop`].
/// A tripwire for bot reply loops, where each wake makes the agent answer a
/// bot that answers back. t3 stops at 10 (`PULL_REQUEST_WATCH_WAKE_LIMIT`,
/// `pullRequestWatch.ts:10-11`); any check, conflict or push news resets
/// the count, as there.
pub(crate) const COMMENT_WAKE_LIMIT: u8 = 10;

/// Refused reads in a row that end a watch as [`End::Unreadable`]: 8 min at
/// the poller's 60 s cadence, t3's limit (`PullRequestWatchReactor.ts:49`).
/// Only a definite refusal counts: `gh` missing or signed out, or a read
/// that answered without this PR (not found, no access). A transport
/// failure or a missed deadline resets the count and keeps the watch, as t3
/// does (`PullRequestWatchReactor.ts:626-629`).
pub(crate) const READ_FAILURE_LIMIT: u8 = 8;

/// What one read of a watched pull request comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Report {
    /// News, in the order the rules run; empty when nothing is worth a turn.
    pub(crate) changes: Vec<Change>,
    /// What the watch remembers for its next read.
    pub(crate) next: WatchState,
    /// Why the watch ends with this read, if it does.
    pub(crate) end: Option<End>,
}

/// What `read`, the pull request's full detail, tells a watch in `watch`.
/// `_now` (unix milliseconds) is unused until a rule needs the clock.
pub(crate) fn evaluate(watch: &WatchState, read: &PrDetail, _now: u64) -> Report {
    let head = &read.summary.change.head_oid;
    let head_moved = watch.head_oid.as_ref().is_some_and(|last| last != head);
    let baseline = watch.baseline == Baseline::Pending;
    let mut next = if head_moved {
        new_head(watch)
    } else {
        watch.clone()
    };
    next.baseline = Baseline::Taken;
    next.head_oid = Some(head.clone());
    next.read_failures = 0;

    let mut changes = checks(&mut next, &read.checks, read.checks_to_come(), baseline);
    changes.extend(remarks(&mut next, read));
    changes.extend(conflict(&mut next, read.mergeable, baseline));
    let comment_loop = comment_loop(&mut next, &changes, head_moved);
    Report {
        changes,
        next,
        end: ended(read).or(comment_loop),
    }
}

/// What a read that failed with `error` tells a watch in `watch`.
pub(crate) fn failed_read(watch: &WatchState, error: &ReadError) -> Report {
    match error {
        ReadError::NotInstalled(why) | ReadError::SignedOut(why) => refused_read(watch, why),
        // The network, the deadline, or GitHub having a bad moment: nothing
        // about this pull request, so the watch carries on.
        ReadError::Failed(_) => Report {
            changes: Vec::new(),
            next: WatchState {
                read_failures: 0,
                ..watch.clone()
            },
            end: None,
        },
    }
}

/// A definite refusal to read the pull request, `why` in the words of `gh`
/// or GitHub: also what the poller passes when a read answered without
/// this PR, as for a repository the account can't see. Ends the watch at
/// [`READ_FAILURE_LIMIT`] in a row.
pub(crate) fn refused_read(watch: &WatchState, why: &str) -> Report {
    let read_failures = watch.read_failures.saturating_add(1);
    Report {
        changes: Vec::new(),
        next: WatchState {
            read_failures,
            ..watch.clone()
        },
        end: (read_failures >= READ_FAILURE_LIMIT).then(|| End::Unreadable(why.to_owned())),
    }
}

/// `watch` for a new head commit: what it knew of the old head's checks
/// goes, and nothing owed from it is told. A conflict the push didn't
/// resolve is told again, as a failed check would be.
fn new_head(watch: &WatchState) -> WatchState {
    WatchState {
        failed: Vec::new(),
        gate_passed: false,
        gate: Vec::new(),
        owed: Owed::Nothing,
        conflicting: false,
        ..watch.clone()
    }
}

/// The check rules. A read without checks keeps what was known: GitHub
/// lists none for a moment after a push, and a PR with none has no news.
/// A failure is told once, when first seen, unless the owed wake that
/// lists every failing check is told in the same read. With checks
/// `to_come` past those the read lists, they may stand any way (see
/// [`gate`]).
fn checks(
    next: &mut WatchState,
    checks: &[CheckRun],
    to_come: bool,
    baseline: bool,
) -> Vec<Change> {
    if checks.is_empty() {
        return Vec::new();
    }
    let failed_before = std::mem::take(&mut next.failed);
    next.failed = checks
        .iter()
        .filter(|check| failed(check).is_some())
        .map(|check| check.name.clone())
        .collect();
    let newly_failed: Vec<CheckRef> = checks
        .iter()
        .filter(|check| !failed_before.contains(&check.name))
        .filter_map(check_ref)
        .collect();
    let (passed, finished) = gate(next, checks, to_come, &failed_before, &newly_failed);

    let mut changes = Vec::new();
    if !baseline && !newly_failed.is_empty() && finished.is_none() {
        changes.push(Change::ChecksFailed {
            failed: newly_failed,
        });
    }
    if !baseline {
        changes.extend(passed);
    }
    changes.extend(finished);
    changes
}

/// The gate rule and the owed wake. The gate is the required checks if any
/// is required, else every check; it passes once every one of them passed,
/// and is told once, or again when a required check joins it. Checks
/// outside a passed gate still running are owed one wake once they finish.
/// A failing gate owes one wake once every check finishes, when other
/// checks still ran as it failed or a failed check ran again (as t3).
/// `next.failed` is already this read's; `failed_before` is the last
/// read's. With checks `to_come` past those listed (unread, or a queued
/// workflow's not made yet), which may be required, running or failing,
/// the gate never passes and nothing is told finished; listed failures
/// still are. Returns the pass and the owed wake, each if
/// told now.
fn gate(
    next: &mut WatchState,
    checks: &[CheckRun],
    to_come: bool,
    failed_before: &[String],
    newly_failed: &[CheckRef],
) -> (Option<Change>, Option<Change>) {
    let (kind, gate) = gate_of(checks);
    let names: Vec<String> = gate.iter().map(|check| check.name.clone()).collect();
    let passed_now = !to_come && gate.iter().all(|check| passes(check.status));
    let grew = kind == Gate::Required && names.iter().any(|name| !next.gate.contains(name));
    let owed_before = std::mem::replace(&mut next.owed, Owed::Nothing);

    let mut passed = None;
    let mut finished = None;
    if passed_now && (!next.gate_passed || grew) {
        passed = Some(Change::ChecksPassed {
            count: count(gate.len()),
            gate: kind,
        });
        let advisory: Vec<String> = checks
            .iter()
            .filter(|check| check.required != Some(true) && running(check.status))
            .map(|check| check.name.clone())
            .collect();
        if kind == Gate::Required && !advisory.is_empty() {
            next.owed = Owed::Advisory(advisory);
        }
    } else if passed_now {
        if let Owed::Advisory(owed) = owed_before {
            // Only owed checks still outside the gate: one turned required
            // is gate news.
            let owed: Vec<String> = owed
                .into_iter()
                .filter(|name| {
                    checks
                        .iter()
                        .any(|check| check.name == *name && check.required != Some(true))
                })
                .collect();
            let settled = owed.iter().all(|name| {
                checks
                    .iter()
                    .all(|check| check.name != *name || !running(check.status))
            });
            if settled && !owed.is_empty() {
                finished = Some(all_finished(checks));
            } else if !owed.is_empty() {
                next.owed = Owed::Advisory(owed);
            }
        }
    } else if gate.iter().any(|check| failed(check).is_some()) {
        // A check that failed before and no longer does ran again.
        let rerun = failed_before.iter().any(|name| !next.failed.contains(name));
        let owing = owed_before != Owed::Nothing || rerun;
        if to_come || checks.iter().any(|check| running(check.status)) {
            if owing || !newly_failed.is_empty() {
                next.owed = Owed::Everything;
            }
        } else if owing {
            finished = Some(all_finished(checks));
        }
    }
    next.gate_passed = passed_now;
    next.gate = if passed_now { names } else { Vec::new() };
    (passed, finished)
}

/// Which checks a pull request must pass to merge: the required ones if
/// any is required, else every check. The evaluator's gate, and the one
/// `watch_pull_request` reports.
pub(super) fn gate_of(checks: &[CheckRun]) -> (Gate, Vec<&CheckRun>) {
    let required: Vec<&CheckRun> = checks
        .iter()
        .filter(|check| check.required == Some(true))
        .collect();
    if required.is_empty() {
        (Gate::All, checks.iter().collect())
    } else {
        (Gate::Required, required)
    }
}

/// The owed wake, told once every check it waited for has finished.
fn all_finished(checks: &[CheckRun]) -> Change {
    Change::ChecksFinished {
        passed: count(
            checks
                .iter()
                .filter(|check| check.status == CheckStatus::Passed)
                .count(),
        ),
        failing: checks.iter().filter_map(check_ref).collect(),
    }
}

/// The remark rule: comments, reviews and comments on lines made since the
/// newest one told, by anyone but the account that reads them (whose
/// replies are the agent's own). Logins compare without case, as GitHub's
/// do. Posts from before the watch began are never told; those since are,
/// even on its first read.
fn remarks(next: &mut WatchState, read: &PrDetail) -> Option<Change> {
    // A read that didn't say who it read as falls back to the PR's author,
    // as t3 does.
    let own = read.viewer.as_deref().unwrap_or(&read.author);
    let through = next.remarks_through;
    let posts = read.posts.iter().map(|post| (None, post));
    let on_lines = read.threads.iter().flat_map(|thread| {
        thread
            .comments
            .iter()
            .map(move |post| (Some(thread.path.as_str()), post))
    });
    let mut fresh: Vec<(Option<&str>, &Post)> = posts
        .chain(on_lines)
        .filter(|(_, post)| {
            let unseen = post.at > through
                || (post.at == through && !next.remark_ids.iter().any(|id| id == key(post)));
            unseen && !post.author.eq_ignore_ascii_case(own)
        })
        .collect();
    fresh.sort_by_key(|(_, post)| post.at);
    let latest = fresh.last()?.1.at;
    if latest > through {
        next.remarks_through = latest;
        next.remark_ids.clear();
    }
    next.remark_ids.extend(
        fresh
            .iter()
            .filter(|(_, post)| post.at == latest)
            .map(|(_, post)| key(post).to_owned()),
    );
    let remarks = fresh
        .into_iter()
        .map(|(path, post)| Remark {
            author: post.author.clone(),
            association: post.author_association.clone(),
            path: path.map(str::to_owned),
            body: post.body.clone(),
            verdict: post.verdict.map(Into::into),
            url: post.url.clone(),
            at: post.at,
        })
        .collect();
    Some(Change::Remarks { remarks })
}

/// A post's identity: its id, or its URL when the read didn't ask for ids.
fn key(post: &Post) -> &str {
    if post.id.is_empty() {
        &post.url
    } else {
        &post.id
    }
}

/// The conflict rule: told when the branch starts to conflict with its
/// base, again only after a clean answer or a new head.
fn conflict(next: &mut WatchState, mergeable: Mergeable, baseline: bool) -> Option<Change> {
    match mergeable {
        Mergeable::Conflicting => {
            let onset = !next.conflicting;
            next.conflicting = true;
            (onset && !baseline).then_some(Change::Conflicting)
        }
        Mergeable::Yes => {
            next.conflicting = false;
            None
        }
        // GitHub is still working it out, as for a while after any push.
        Mergeable::Unknown => None,
    }
}

/// A merge or close ends the watch.
fn ended(read: &PrDetail) -> Option<End> {
    match read.summary.state {
        PrState::Open | PrState::Draft => None,
        PrState::Merged => Some(End::Merged {
            by: read.merged_by.clone(),
            commit: read.merge_commit.clone(),
            base: read.base.clone(),
        }),
        PrState::Closed => Some(End::Closed),
    }
}

/// Counts comment-only reports in a row, ending the watch at
/// [`COMMENT_WAKE_LIMIT`]. Any other news, or a push, starts the count over.
fn comment_loop(next: &mut WatchState, changes: &[Change], head_moved: bool) -> Option<End> {
    let comments_only = !changes.is_empty()
        && changes
            .iter()
            .all(|change| matches!(change, Change::Remarks { .. }));
    if head_moved || (!changes.is_empty() && !comments_only) {
        next.comment_wakes = 0;
    }
    if comments_only {
        next.comment_wakes = next.comment_wakes.saturating_add(1);
    }
    (comments_only && next.comment_wakes >= COMMENT_WAKE_LIMIT).then_some(End::CommentLoop)
}

/// How `check` didn't pass, if it finished without passing.
fn failed(check: &CheckRun) -> Option<Failure> {
    match check.status {
        CheckStatus::Failed => Some(Failure::Failed),
        CheckStatus::Cancelled => Some(Failure::Cancelled),
        CheckStatus::Running
        | CheckStatus::Pending
        | CheckStatus::Passed
        | CheckStatus::Neutral
        | CheckStatus::Skipped => None,
    }
}

/// `check` as news, if it didn't pass.
fn check_ref(check: &CheckRun) -> Option<CheckRef> {
    Some(CheckRef {
        name: check.name.clone(),
        url: check.url.clone(),
        failure: failed(check)?,
    })
}

/// Whether a check in `status` has yet to finish.
pub(super) fn running(status: CheckStatus) -> bool {
    matches!(status, CheckStatus::Running | CheckStatus::Pending)
}

/// Whether a check in `status` lets the gate pass: it finished, and didn't
/// fail.
fn passes(status: CheckStatus) -> bool {
    match status {
        CheckStatus::Passed | CheckStatus::Neutral | CheckStatus::Skipped => true,
        CheckStatus::Failed
        | CheckStatus::Cancelled
        | CheckStatus::Running
        | CheckStatus::Pending => false,
    }
}

fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
