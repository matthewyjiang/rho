//! A session's pull requests: the one its workspace's branch has, and those
//! linked to it, which a client lists for many sessions at once, shows one
//! of, and links or unlinks. The daemon reads GitHub; clients only ask.
//!
//! The daemon reads a pull request while something wants it: a session
//! that watches it, or a client that listed or showed it recently. One
//! that is only linked it reads every 5 minutes until it merges or closes.
//! Each
//! `list` and `show` takes a lease on what it names, which runs out unless
//! renewed: a `list` lease lasts 10 minutes and a `show` lease 60 seconds,
//! two periods of the cadence each is read at (5 minutes and 30 seconds).
//! Reissue `list` every 5 minutes and `show` every 30 seconds, so one late
//! request does not stop the reads.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PullRequestDetail, SessionId};

/// List the pull requests of each session named, as the daemon last read
/// them, and keep reading them every 5 minutes: the request leases each
/// session for 10 minutes, so a client reissues it every 5 minutes to keep
/// its rows read. Asks for more than
/// [`SessionPullRequestListParams::MAX_SESSIONS`] sessions are
/// [`crate::ErrorCode::InvalidParams`], naming the limit and the count
/// asked. A daemon from before this method
/// answers [`crate::ErrorCode::MethodNotFound`].
pub const SESSION_PULL_REQUEST_LIST: &str = "rho/session/pull_request/list";

/// Show one of a session's pull requests: its set, and the detail of the
/// target. Answers from the daemon's last read; it reads GitHub first when
/// `read` is [`PullRequestRead::Fresh`], or when it has no detail of the
/// target as new as its summary to answer with, unless its last attempt to
/// read it failed within the last 15 seconds. Leases the target for 60
/// seconds, in which the daemon reads it every 30 seconds; a client
/// reissues it every 30 seconds while the target is on screen. A read that
/// finds the summary unchanged and no checks running keeps the last full
/// detail, read in full again at least every 10 minutes: a client that
/// needs a new full read sends [`PullRequestRead::Fresh`].
/// An unknown session is [`crate::ErrorCode::SessionNotFound`]; a linked
/// target that is not linked to the session, or names no pull request, is
/// [`crate::ErrorCode::InvalidParams`].
pub const SESSION_PULL_REQUEST_SHOW: &str = "rho/session/pull_request/show";

/// Link a pull request to a session, as the agent's `link_pull_request`
/// tool does, and save it with the session. A pull request already linked
/// stays linked once. Only a pull request on github.com, or on the host of
/// the session's `origin`, can be linked: text that names no pull request,
/// or one on another host, is [`crate::ErrorCode::InvalidParams`]; an
/// unknown session is [`crate::ErrorCode::SessionNotFound`].
pub const SESSION_PULL_REQUEST_LINK: &str = "rho/session/pull_request/link";

/// Unlink a pull request from a session, ending a watch on it. Unlinking
/// one that is not linked succeeds. Refusals are those of
/// [`SESSION_PULL_REQUEST_LINK`].
pub const SESSION_PULL_REQUEST_UNLINK: &str = "rho/session/pull_request/unlink";

/// Params of [`SESSION_PULL_REQUEST_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestListParams {
    /// The sessions to list and lease, at most 1,000.
    #[schemars(length(max = SessionPullRequestListParams::MAX_SESSIONS))]
    pub sessions: Vec<SessionId>,
}

impl SessionPullRequestListParams {
    /// The most sessions one request names: the bound of a
    /// `rho/session/list` page, so a client leases every row of a page in
    /// one request. The daemon refuses more with
    /// [`crate::ErrorCode::InvalidParams`].
    pub const MAX_SESSIONS: usize = 1_000;

    /// Builds it from each of its fields.
    pub fn new(sessions: Vec<SessionId>) -> Self {
        Self { sessions }
    }
}

/// Result of [`SESSION_PULL_REQUEST_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestListResponse {
    /// In the order asked. An id the daemon has nothing loaded or saved
    /// under, one that is not a session id, and one it could not look up
    /// are left out.
    pub sessions: Vec<ListedSessionPullRequests>,
}

impl SessionPullRequestListResponse {
    /// Builds it from each of its fields.
    pub fn new(sessions: Vec<ListedSessionPullRequests>) -> Self {
        Self { sessions }
    }
}

/// One session's pull requests in a [`SessionPullRequestListResponse`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ListedSessionPullRequests {
    pub session: SessionId,
    pub pull_requests: SessionPullRequests,
}

impl ListedSessionPullRequests {
    /// Builds it from each of its fields.
    pub fn new(session: SessionId, pull_requests: SessionPullRequests) -> Self {
        Self {
            session,
            pull_requests,
        }
    }
}

/// Params of [`SESSION_PULL_REQUEST_SHOW`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestShowParams {
    pub session: SessionId,
    pub target: PullRequestTarget,
    /// Whether to read GitHub before answering; absent, `cached`.
    #[serde(default, skip_serializing_if = "PullRequestRead::is_cached")]
    pub read: PullRequestRead,
}

impl SessionPullRequestShowParams {
    /// Params that answer from the daemon's last read.
    pub fn new(session: SessionId, target: PullRequestTarget) -> Self {
        Self {
            session,
            target,
            read: PullRequestRead::Cached,
        }
    }
}

/// Result of [`SESSION_PULL_REQUEST_SHOW`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestShowResponse {
    pub pull_requests: SessionPullRequests,
    /// The target's detail: absent while the daemon has none, as for a
    /// branch with no pull request or a read that failed (see the target's
    /// [`PullRequestTrouble`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Box<PullRequestDetail>>,
}

impl SessionPullRequestShowResponse {
    /// Builds it from each of its fields.
    pub fn new(pull_requests: SessionPullRequests, detail: Option<Box<PullRequestDetail>>) -> Self {
        Self {
            pull_requests,
            detail,
        }
    }
}

/// Params of [`SESSION_PULL_REQUEST_LINK`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestLinkParams {
    pub session: SessionId,
    /// The pull request as a person typed it: a URL, `owner/name#12`, or
    /// `#12` in the repository of the session's `origin`.
    pub pull_request: String,
}

impl SessionPullRequestLinkParams {
    /// Builds it from each of its fields.
    pub fn new(session: SessionId, pull_request: impl Into<String>) -> Self {
        Self {
            session,
            pull_request: pull_request.into(),
        }
    }
}

/// Result of [`SESSION_PULL_REQUEST_LINK`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestLinkResponse {
    /// The pull request the text named.
    pub pull_request: PullRequestId,
    pub pull_requests: SessionPullRequests,
}

impl SessionPullRequestLinkResponse {
    /// Builds it from each of its fields.
    pub fn new(pull_request: PullRequestId, pull_requests: SessionPullRequests) -> Self {
        Self {
            pull_request,
            pull_requests,
        }
    }
}

/// Params of [`SESSION_PULL_REQUEST_UNLINK`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestUnlinkParams {
    pub session: SessionId,
    /// The pull request, written as [`SessionPullRequestLinkParams`] takes it.
    pub pull_request: String,
}

impl SessionPullRequestUnlinkParams {
    /// Builds it from each of its fields.
    pub fn new(session: SessionId, pull_request: impl Into<String>) -> Self {
        Self {
            session,
            pull_request: pull_request.into(),
        }
    }
}

/// Result of [`SESSION_PULL_REQUEST_UNLINK`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequestUnlinkResponse {
    pub pull_requests: SessionPullRequests,
}

impl SessionPullRequestUnlinkResponse {
    /// Builds it from each of its fields.
    pub fn new(pull_requests: SessionPullRequests) -> Self {
        Self { pull_requests }
    }
}

/// Which of a session's pull requests [`SESSION_PULL_REQUEST_SHOW`] shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestTarget {
    /// The pull request of the branch the session's workspace has checked
    /// out, if it has one.
    Branch,
    /// One linked to the session, written as [`SessionPullRequestLinkParams`]
    /// takes it.
    Linked { pull_request: String },
}

/// Whether [`SESSION_PULL_REQUEST_SHOW`] reads GitHub before answering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestRead {
    /// Answer from the daemon's last read, reading first only when it has
    /// no detail of the target to answer with and its last attempt did not
    /// fail within the last 15 seconds.
    #[default]
    Cached,
    /// Read now, as after a merge or comment the client made itself.
    Fresh,
}

impl PullRequestRead {
    fn is_cached(&self) -> bool {
        *self == Self::Cached
    }
}

/// A session's pull requests: its branch's, and those linked to it. What
/// [`SESSION_PULL_REQUEST_LIST`] gives per session, what the other methods
/// answer with, and what [`super::SessionEvent::PullRequests`] carries: a
/// client replaces what it holds with it, never merges.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionPullRequests {
    /// The branch the session's workspace has checked out, and its pull
    /// request. Absent until a client listed or showed the session since
    /// the daemon started; kept as last read once no client asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<BranchPullRequest>,
    /// In the order linked, each once. The branch's own pull request is here
    /// too when it is linked, as a watch links it: match by `id`.
    pub linked: Vec<LinkedPullRequest>,
}

impl SessionPullRequests {
    /// Builds it from each of its fields.
    pub fn new(branch: Option<BranchPullRequest>, linked: Vec<LinkedPullRequest>) -> Self {
        Self { branch, linked }
    }

    /// The pull request the branch's lookup found, if it found one.
    pub fn branch_pull_request(&self) -> Option<&PullRequestSummary> {
        match &self.branch.as_ref()?.lookup {
            BranchLookup::Found { pull_request } => Some(pull_request),
            BranchLookup::NotGit
            | BranchLookup::Detached { .. }
            | BranchLookup::NoForge
            | BranchLookup::DefaultBranch
            | BranchLookup::Looking
            | BranchLookup::None
            | BranchLookup::OnlyForks { .. }
            | BranchLookup::Unknown => None,
        }
    }

    /// The link to pull request `id`, if the session links it.
    pub fn link(&self, id: &PullRequestId) -> Option<&LinkedPullRequest> {
        self.linked.iter().find(|linked| linked.id == *id)
    }
}

/// The branch a session's workspace has checked out, and what looking up
/// its pull request found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct BranchPullRequest {
    /// Absent when the checkout is on no branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub lookup: BranchLookup,
    /// Why `lookup` may be out of date, or still `looking`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trouble: Option<PullRequestTrouble>,
}

impl BranchPullRequest {
    /// Builds it from each of its fields.
    pub fn new(
        branch: Option<String>,
        lookup: BranchLookup,
        trouble: Option<PullRequestTrouble>,
    ) -> Self {
        Self {
            branch,
            lookup,
            trouble,
        }
    }
}

/// What looking up a branch's pull request found. Protocol minors may add
/// kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BranchLookup {
    /// The workspace is in no git repository.
    NotGit,
    /// The checkout is on no branch, at `commit` (abbreviated).
    Detached { commit: String },
    /// The repository has no `origin` on github.com, the one host whose
    /// branches the daemon looks up.
    NoForge,
    /// The branch is the repository's default branch, which has no pull
    /// request of its own; nothing is looked up.
    DefaultBranch,
    /// Not answered yet.
    Looking,
    /// The branch has no pull request.
    None,
    /// The newest pull requests of the branch, `count` of them, all came
    /// from forks.
    OnlyForks { count: u32 },
    Found {
        pull_request: Box<PullRequestSummary>,
    },
    /// A kind a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A pull request linked to a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct LinkedPullRequest {
    pub id: PullRequestId,
    pub url: String,
    pub origin: PullRequestLinkOrigin,
    /// When it was linked, in unix seconds.
    pub linked_at: u64,
    /// Whether the session's agent watches it: the daemon wakes the agent
    /// when it changes.
    pub watched: bool,
    /// Absent before the daemon first read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<PullRequestSummary>,
    /// Why `summary` may be out of date, or absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trouble: Option<PullRequestTrouble>,
}

impl LinkedPullRequest {
    /// A link not watched and not read yet.
    pub fn new(
        id: PullRequestId,
        url: impl Into<String>,
        origin: PullRequestLinkOrigin,
        linked_at: u64,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            origin,
            linked_at,
            watched: false,
            summary: None,
            trouble: None,
        }
    }
}

/// Who linked a pull request. Protocol minors may add origins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestLinkOrigin {
    /// A person, through a client.
    User,
    /// The session's agent, through its tools.
    Agent,
    /// An origin a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A pull request's identity. Shown as `owner/name#12`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PullRequestId {
    /// The forge's host, such as `github.com`.
    pub host: String,
    pub owner: String,
    /// The repository's name.
    pub name: String,
    pub number: u32,
}

impl PullRequestId {
    /// Builds it from each of its fields.
    pub fn new(
        host: impl Into<String>,
        owner: impl Into<String>,
        name: impl Into<String>,
        number: u32,
    ) -> Self {
        Self {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
            number,
        }
    }
}

impl fmt::Display for PullRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}#{}", self.owner, self.name, self.number)
    }
}

/// Enough of a pull request for a sidebar row or a tab title, and to tell
/// that its detail changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PullRequestSummary {
    pub id: PullRequestId,
    pub title: String,
    pub url: String,
    pub state: PullRequestState,
    /// Its checks, rolled up.
    pub rollup: PullRequestRollup,
    /// The commit its head branch is at.
    pub head_oid: String,
    /// The forge's timestamp of its last update. When it, `head_oid`, or
    /// `rollup` moved, a [`SESSION_PULL_REQUEST_SHOW`] returns a newer
    /// detail.
    pub updated_at: String,
}

impl PullRequestSummary {
    /// Builds it from each of its fields.
    pub fn new(
        id: PullRequestId,
        title: impl Into<String>,
        url: impl Into<String>,
        state: PullRequestState,
        rollup: PullRequestRollup,
        head_oid: impl Into<String>,
        updated_at: impl Into<String>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            url: url.into(),
            state,
            rollup,
            head_oid: head_oid.into(),
            updated_at: updated_at.into(),
        }
    }
}

/// Where a pull request is. Protocol minors may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestState {
    Open,
    Draft,
    Merged,
    Closed,
    /// A state a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// A pull request's checks, rolled up. Protocol minors may add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestRollup {
    /// It has no checks.
    None,
    Pending,
    Passing,
    Failing,
    /// A value a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Why the daemon could not read a pull request or a branch: what it holds
/// is its last good read, if any. Protocol minors may add kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PullRequestTrouble {
    /// The daemon's machine has no `gh`.
    NotInstalled,
    /// `gh` on the daemon's machine is not signed in to `host`.
    SignedOut { host: String },
    /// Reads of `host` are paused to keep its API budget: `remaining`
    /// points are left, under the daemon's `floor`, until `resets_at`
    /// (unix seconds).
    Paused {
        host: String,
        remaining: u32,
        floor: u32,
        resets_at: i64,
    },
    /// The forge answered without it, as its `message` says.
    NotFound { message: String },
    /// Any other failure.
    Failed { message: String },
    /// A kind a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
