//! What [`super::SESSION_PULL_REQUEST_SHOW`] answers about one pull
//! request: its description, checks, conversation and review threads. Only
//! that answer carries it, since a detail is tens of kilobytes where a
//! [`PullRequestSummary`] is a few hundred bytes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PullRequestSummary;

/// One pull request as the daemon last read it whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PullRequestDetail {
    pub summary: PullRequestSummary,
    /// When the daemon read it, in unix seconds.
    pub read_at: u64,
    /// The login that opened it.
    pub author: String,
    /// The branch it merges from.
    pub head: String,
    /// The branch it merges into.
    pub base: String,
    /// Its description, in Markdown.
    pub body: String,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    pub review: PullRequestReview,
    pub mergeable: PullRequestMergeable,
    /// How the repository lets the reader merge it, the reader's default
    /// first. Empty when the read did not say.
    pub merge_methods: Vec<PullRequestMergeMethod>,
    /// Its first checks, those that need a look first: failed, running,
    /// pending, cancelled, passed, neutral, skipped. `checks_total` counts
    /// them all, which may be more.
    pub checks: Vec<PullRequestCheck>,
    pub checks_total: u32,
    /// Its newest comments and reviews, oldest first; `older_posts` older
    /// ones were not read.
    pub posts: Vec<PullRequestPost>,
    pub older_posts: u32,
    /// Its newest threads on lines of the code; `older_threads` older ones
    /// were not read.
    pub threads: Vec<PullRequestThread>,
    pub older_threads: u32,
    /// The login the daemon read it as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<String>,
    /// Who merged it, once merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_by: Option<String>,
    /// The commit it merged as, once merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
}

impl PullRequestDetail {
    /// A detail of `summary` read at `read_at` with everything else empty:
    /// no text, counts of 0, no review asked, mergeability undetermined.
    pub fn new(summary: PullRequestSummary, read_at: u64) -> Self {
        Self {
            summary,
            read_at,
            author: String::new(),
            head: String::new(),
            base: String::new(),
            body: String::new(),
            additions: 0,
            deletions: 0,
            changed_files: 0,
            review: PullRequestReview::None,
            mergeable: PullRequestMergeable::Undetermined,
            merge_methods: Vec::new(),
            checks: Vec::new(),
            checks_total: 0,
            posts: Vec::new(),
            older_posts: 0,
            threads: Vec::new(),
            older_threads: 0,
            viewer: None,
            merged_by: None,
            merge_commit: None,
        }
    }
}

/// Where a pull request's review is. Protocol minors may add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestReview {
    /// No review is required or given.
    None,
    Required,
    ChangesRequested,
    Approved,
    /// A value a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Whether a pull request merges into its base. Protocol minors may add
/// values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestMergeable {
    Clean,
    Conflicting,
    /// The forge has not worked it out yet, or the pull request is closed.
    Undetermined,
    /// A value a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A way to merge a pull request. Protocol minors may add methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestMergeMethod {
    Merge,
    Squash,
    Rebase,
    /// A method a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One check run on a pull request's head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PullRequestCheck {
    pub name: String,
    /// The workflow it belongs to, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    pub status: PullRequestCheckStatus,
    /// Its log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Whether the base branch requires it; absent when the read did not
    /// say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

impl PullRequestCheck {
    /// Builds it from each of its fields.
    pub fn new(
        name: impl Into<String>,
        workflow: Option<String>,
        status: PullRequestCheckStatus,
        url: Option<String>,
        required: Option<bool>,
    ) -> Self {
        Self {
            name: name.into(),
            workflow,
            status,
            url,
            required,
        }
    }
}

/// Where a check is. Protocol minors may add statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestCheckStatus {
    Failed,
    Running,
    Pending,
    Cancelled,
    Passed,
    Neutral,
    Skipped,
    /// A status a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A comment or review on a pull request, or a comment in one of its
/// threads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PullRequestPost {
    /// The forge's id of it; empty when the read did not ask.
    pub id: String,
    /// The login that wrote it.
    pub author: String,
    /// The author's tie to the repository, as the forge names it, such as
    /// `MEMBER`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_association: Option<String>,
    /// When it was written, in unix seconds.
    pub at: i64,
    /// In Markdown; a review may have none.
    pub body: String,
    /// A review's verdict; absent for a comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<PullRequestVerdict>,
    pub url: String,
}

impl PullRequestPost {
    /// A comment with no verdict and no author association.
    pub fn new(
        id: impl Into<String>,
        author: impl Into<String>,
        at: i64,
        body: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            author: author.into(),
            author_association: None,
            at,
            body: body.into(),
            verdict: None,
            url: url.into(),
        }
    }
}

/// What a review said. Protocol minors may add verdicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestVerdict {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    /// A verdict a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A review thread on a line of a pull request's diff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PullRequestThread {
    /// The forge's id of it.
    pub id: String,
    /// The file it is on.
    pub path: String,
    /// The line it is on, which an outdated thread no longer has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub state: PullRequestThreadState,
    /// The comments that start it and its newest, oldest first and each
    /// once; `more_comments` between those were not read.
    pub comments: Vec<PullRequestPost>,
    pub more_comments: u32,
}

impl PullRequestThread {
    /// Builds it from each of its fields.
    pub fn new(
        id: impl Into<String>,
        path: impl Into<String>,
        line: Option<u32>,
        state: PullRequestThreadState,
        comments: Vec<PullRequestPost>,
        more_comments: u32,
    ) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            line,
            state,
            comments,
            more_comments,
        }
    }
}

/// Where a review thread is. Protocol minors may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestThreadState {
    Open,
    /// Open, on code that has changed since.
    Outdated,
    Resolved,
    /// A state a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
