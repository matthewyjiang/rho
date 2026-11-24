//! What the PR service keeps on disk for a watch: the evaluator's memory of
//! a pull request ([`WatchState`]), and the news it finds there ([`News`])
//! until a turn has taken it. [`super::watch`] computes both; [`super::news`]
//! renders news for the model and for clients.
//!
//! Times are what callers pass in: unix milliseconds for when rho did
//! something, and the forge's unix seconds for when a post was made.

use serde::{Deserialize, Serialize};

use super::model::{CheckStatus, PrDetail, PrId, Verdict};

/// What one watch remembers of its pull request between reads, so that
/// [`super::watch::evaluate`] tells each change once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WatchState {
    /// When the watch began, in unix milliseconds.
    pub(crate) started_at: u64,
    /// Whether the first read has been taken; see [`Baseline`].
    pub(crate) baseline: Baseline,
    /// The head commit last read; checks belong to it.
    pub(crate) head_oid: Option<String>,
    /// Names of the checks failing at the last read.
    pub(crate) failed: Vec<String>,
    /// Whether the gate passed at the last read.
    pub(crate) gate_passed: bool,
    /// Names of the gate's checks when it passed, so that a required check
    /// added later is told as a new pass; empty while it hasn't passed.
    pub(crate) gate: Vec<String>,
    /// The one check wake still owed.
    pub(crate) owed: Owed,
    /// The newest post time told, in the forge's unix seconds.
    pub(crate) remarks_through: i64,
    /// Ids of the posts told at `remarks_through`: the forge stamps posts to
    /// the second, so posts at the boundary are told apart by id.
    pub(crate) remark_ids: Vec<String>,
    /// Whether a conflict with the base branch has been told.
    pub(crate) conflicting: bool,
    /// Comment-only reports in a row; see `watch::COMMENT_WAKE_LIMIT`.
    pub(crate) comment_wakes: u8,
    /// Refused reads in a row; see `watch::READ_FAILURE_LIMIT`.
    pub(crate) read_failures: u8,
}

impl WatchState {
    /// A watch begun at `started_at` (unix milliseconds), before its first
    /// read. Posts from the second it began on count as news.
    pub(crate) fn new(started_at: u64) -> Self {
        Self {
            started_at,
            baseline: Baseline::Pending,
            head_oid: None,
            failed: Vec::new(),
            gate_passed: false,
            gate: Vec::new(),
            owed: Owed::Nothing,
            remarks_through: i64::try_from(started_at / 1000).unwrap_or(i64::MAX),
            remark_ids: Vec::new(),
            conflicting: false,
            comment_wakes: 0,
            read_failures: 0,
        }
    }
}

/// Whether a watch's first read has been taken. That read only records the
/// checks and conflicts already there, which the agent saw when it began
/// the watch; posts made since the watch began are still news.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Baseline {
    Pending,
    Taken,
}

/// A [`Change::ChecksFinished`] wake promised for later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Owed {
    Nothing,
    /// These checks outside the required gate were still running when it
    /// passed: one wake once they have all finished.
    Advisory(Vec<String>),
    /// The gate failed while other checks still ran: one wake once every
    /// check has finished.
    Everything,
}

/// Something about a watched pull request worth a turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Change {
    /// Checks that failed since the last read; not those failing before.
    ChecksFailed { failed: Vec<CheckRef> },
    /// Every check of the gate passed: `count` of them.
    ChecksPassed { count: u32, gate: Gate },
    /// The owed wake: every check it waited for has finished.
    ChecksFinished { passed: u32, failing: Vec<CheckRef> },
    /// New comments and reviews by others, oldest first.
    Remarks { remarks: Vec<Remark> },
    /// The branch stopped merging cleanly into its base.
    Conflicting,
}

/// Which checks a pull request must pass to merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Gate {
    /// The base branch's protection requires these.
    Required,
    /// None is required, so every check counts.
    All,
}

/// A check that didn't pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CheckRef {
    pub(crate) name: String,
    pub(crate) url: Option<String>,
    pub(crate) failure: Failure,
}

/// How a check didn't pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Failure {
    Failed,
    Cancelled,
}

/// A comment or review by someone else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Remark {
    pub(crate) author: String,
    /// Their standing in the repository as GitHub names it, such as
    /// `MEMBER`; `None` when the read didn't say.
    pub(crate) association: Option<String>,
    /// The file it is on, for a comment on a line.
    pub(crate) path: Option<String>,
    /// Markdown, as written: untrusted text.
    pub(crate) body: String,
    /// What a review decided; `None` for a comment.
    pub(crate) verdict: Option<Reviewed>,
    pub(crate) url: String,
    /// The forge's unix seconds.
    pub(crate) at: i64,
}

/// What a review decided; [`Verdict`] as it is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Reviewed {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

impl From<Verdict> for Reviewed {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Approved => Self::Approved,
            Verdict::ChangesRequested => Self::ChangesRequested,
            Verdict::Commented => Self::Commented,
            Verdict::Dismissed => Self::Dismissed,
        }
    }
}

/// Why a watch ended by itself. Each is told once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum End {
    /// Merged into `base`, by `by`, as `commit`, when the read said.
    Merged {
        by: Option<String>,
        commit: Option<String>,
        base: String,
    },
    /// Closed without merging.
    Closed,
    /// Too many comment-only wakes in a row: likely a bot reply loop.
    CommentLoop,
    /// Too many refused reads in a row, with the last refusal's words.
    Unreadable(String),
}

/// When rho found the newest merge `news` tells of, in unix milliseconds:
/// `None` when it tells of none, and when a watch ended some other way
/// then or since, so a pull request closed unmerged beside a merged one
/// does not read as work that all merged.
pub(super) fn merged_last<'a>(news: impl IntoIterator<Item = &'a News> + Clone) -> Option<u64> {
    let merged = news
        .clone()
        .into_iter()
        .filter(|news| matches!(news.end, Some(End::Merged { .. })))
        .map(|news| news.at)
        .max();
    merged.filter(|merged| !ended_otherwise_since(news, *merged))
}

/// Whether `news` tells of a watch that ended some other way than merged,
/// found at `since` (unix milliseconds) or later.
pub(super) fn ended_otherwise_since<'a>(
    news: impl IntoIterator<Item = &'a News>,
    since: u64,
) -> bool {
    news.into_iter().any(|news| match news.end {
        Some(End::Closed | End::CommentLoop | End::Unreadable(_)) => news.at >= since,
        Some(End::Merged { .. }) | None => false,
    })
}

/// One evaluation's news on one pull request, queued for the session until
/// a turn takes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct News {
    /// Its place in its session's queue, unique there: set by
    /// [`super::record::SessionRecord::queue_news`], which ignores what
    /// the caller put here. In-flight marks and acknowledgements name news
    /// by it.
    #[serde(default)]
    pub(crate) seq: u64,
    pub(crate) pr: PrId,
    pub(crate) url: String,
    /// The head commit the changes are about, when known.
    pub(crate) head: Option<String>,
    pub(crate) changes: Vec<Change>,
    pub(crate) end: Option<End>,
    /// When rho found it, in unix milliseconds.
    pub(crate) at: u64,
    /// Whether the head moved since the agent was last told of this pull
    /// request, so check results it heard of no longer apply.
    #[serde(default)]
    pub(crate) pushed: bool,
    /// Where the head's checks stood at the read, for a wake that tells of
    /// failures while others still run.
    #[serde(default)]
    pub(crate) checks: Option<CheckTally>,
}

/// How a read's checks stood, by outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CheckTally {
    /// Passed, neutral or skipped.
    pub(crate) passed: u32,
    /// Running or waiting to run.
    pub(crate) running: u32,
    /// Failed or cancelled.
    pub(crate) failed: u32,
    /// Past the checks the read listed, so of unknown outcome.
    pub(crate) unread: u32,
}

impl CheckTally {
    pub(crate) fn of(detail: &PrDetail) -> Self {
        let mut tally = Self {
            unread: detail.unread_checks(),
            ..Self::default()
        };
        for check in &detail.checks {
            let count = match check.status {
                CheckStatus::Passed | CheckStatus::Neutral | CheckStatus::Skipped => {
                    &mut tally.passed
                }
                CheckStatus::Running | CheckStatus::Pending => &mut tally.running,
                CheckStatus::Failed | CheckStatus::Cancelled => &mut tally.failed,
            };
            *count += 1;
        }
        tally
    }
}

impl News {
    /// Whether the watch ended with this news.
    pub(crate) fn is_end(&self) -> bool {
        self.end.is_some()
    }
}
