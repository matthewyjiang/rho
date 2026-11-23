//! What a pull request is, as rho reads it from a forge: the repositories,
//! branches and PRs it names, what a read finds, and what one batched query
//! asks and answers. Plain data, read by [`super::gh_query`] and consumed by
//! the PR service.

use serde::{Deserialize, Serialize};

/// The host every session may read pull requests on, and the only one a
/// branch's pull request is looked up on.
pub(crate) const GITHUB: &str = "github.com";

/// A repository on a forge, named as its origin remote's URL names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct RepoId {
    /// Such as `github.com`, in lowercase.
    pub(crate) host: String,
    pub(crate) owner: String,
    pub(crate) name: String,
}

impl RepoId {
    /// The repository a remote `url` names: `https://host/owner/name`,
    /// `ssh://git@host:22/owner/name.git`, or `git@host:owner/name.git`.
    /// `None` for a local path or a URL without an owner.
    pub(crate) fn from_url(url: &str) -> Option<Self> {
        let url = url.trim().trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        // `scheme://[user@]host[:port]/path`, or scp's `[user@]host:path`.
        let (host, path) = if let Some((_, rest)) = url.split_once("://") {
            let (authority, path) = rest.split_once('/')?;
            let host = without_user(authority);
            (host.split_once(':').map_or(host, |(host, _)| host), path)
        } else {
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            (without_user(authority), path)
        };
        let mut segments = path.rsplit('/').filter(|segment| !segment.is_empty());
        let name = segments.next()?;
        let owner = segments.next()?;
        if host.is_empty() {
            return None;
        }
        Some(Self {
            host: host.to_ascii_lowercase(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }
}

/// `host` from a URL's `[user@]host`.
fn without_user(authority: &str) -> &str {
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
}

/// A branch of a repository, which may have a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BranchId {
    pub(crate) repo: RepoId,
    pub(crate) branch: String,
}

/// A pull request, by its repository and number.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct PrId {
    pub(crate) repo: RepoId,
    pub(crate) number: u32,
}

impl std::fmt::Display for PrId {
    /// `owner/name#12`, as GitHub names a PR.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}#{}", self.repo.owner, self.repo.name, self.number)
    }
}

impl PrId {
    /// Where the PR shows on its forge: `https://host/owner/name/pull/12`.
    pub(crate) fn url(&self) -> String {
        let RepoId { host, owner, name } = &self.repo;
        format!("https://{host}/{owner}/{name}/pull/{}", self.number)
    }

    /// The PR `text` names: its URL (`https://github.com/owner/name/pull/12`),
    /// `owner/name#12`, or `#12` or `12` in `repo`, the workspace's origin.
    ///
    /// # Errors
    ///
    /// When `text` names no PR, or only a number with no `repo`: why.
    pub(crate) fn parse(text: &str, repo: Option<&RepoId>) -> Result<Self, &'static str> {
        const FORMS: &str = "Give the pull request's URL, owner/name#number, or #number.";
        // Such as a GitLab merge request's or a GitHub issue's URL.
        const NOT_A_PR_URL: &str = "That URL is not a GitHub pull request's; rho reads GitHub \
                                    pull requests only, by a URL such as \
                                    https://github.com/owner/name/pull/12.";
        let text = text.trim();
        let number = |text: &str| text.parse::<u32>().ok().filter(|&number| number > 0);
        if let Some((_, rest)) = text.split_once("://") {
            let mut parts = rest.split('/');
            let (Some(host), Some(owner), Some(name), Some("pull"), Some(n)) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) else {
                return Err(NOT_A_PR_URL);
            };
            let number = number(n.split(['#', '?']).next().unwrap_or(n)).ok_or(NOT_A_PR_URL)?;
            let repo = RepoId {
                host: without_user(host).to_ascii_lowercase(),
                owner: owner.to_owned(),
                name: name.to_owned(),
            };
            return Ok(Self { repo, number });
        }
        if let Some((repo_name, n)) = text.split_once('#').filter(|(name, _)| !name.is_empty()) {
            let (owner, name) = repo_name.split_once('/').ok_or(FORMS)?;
            let host = repo.map_or("github.com", |repo| repo.host.as_str());
            let repo = RepoId {
                host: host.to_owned(),
                owner: owner.to_owned(),
                name: name.to_owned(),
            };
            return Ok(Self {
                repo,
                number: number(n).ok_or(FORMS)?,
            });
        }
        let number = number(text.trim_start_matches('#')).ok_or(FORMS)?;
        let repo =
            repo.ok_or("This session's folder has no repository on a forge: give the URL.")?;
        Ok(Self {
            repo: repo.clone(),
            number,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

impl PrState {
    /// The state as the PR tools' text names it.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// What a PR's checks come to, together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rollup {
    /// It has none.
    None,
    Pending,
    Passing,
    Failing,
}

/// What changes when anything about a PR does: its last update, its head
/// commit, and its checks, which change neither. A detail read with the
/// same key would read the same, except for what GitHub works out on its
/// own, such as whether it still merges cleanly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChangeKey {
    pub(crate) updated_at: String,
    pub(crate) head_oid: String,
    pub(crate) rollup: Rollup,
}

/// What a cheap read tells of a PR: enough to tell whether it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrSummary {
    pub(crate) id: PrId,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) state: PrState,
    pub(crate) change: ChangeKey,
}

/// Whether a PR's reviews let it merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Review {
    /// The repository asks for none, or none came.
    None,
    Required,
    ChangesRequested,
    Approved,
}

/// How one check stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CheckStatus {
    // In the order a detail lists them: what needs a look first.
    Failed,
    Running,
    Pending,
    Cancelled,
    Passed,
    Neutral,
    Skipped,
}

/// One check of a PR's head commit: a CI job, or a commit status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckRun {
    pub(crate) name: String,
    /// The CI workflow it ran in, if any.
    pub(crate) workflow: Option<String>,
    pub(crate) status: CheckStatus,
    /// Where it shows its log or result.
    pub(crate) url: Option<String>,
    /// Whether the base branch's protection requires it to pass before the
    /// PR merges; `None` when the read didn't say.
    pub(crate) required: Option<bool>,
}

/// Everything a full read tells of a PR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrDetail {
    pub(crate) summary: PrSummary,
    pub(crate) author: String,
    pub(crate) head: String,
    pub(crate) base: String,
    /// Its description, as Markdown.
    pub(crate) body: String,
    pub(crate) additions: u32,
    pub(crate) deletions: u32,
    pub(crate) changed_files: u32,
    pub(crate) review: Review,
    /// The first checks, those that need a look first; see
    /// [`Self::checks_total`].
    pub(crate) checks: Vec<CheckRun>,
    /// How many checks there are, which may be more than `checks` holds.
    pub(crate) checks_total: u32,
    /// The newest comments and reviews, oldest first.
    pub(crate) posts: Vec<Post>,
    /// Comments and reviews older than `posts`, not read.
    pub(crate) older_posts: u32,
    /// The newest comment threads on lines of the code.
    pub(crate) threads: Vec<Thread>,
    /// Threads older than `threads`, not read.
    pub(crate) older_threads: u32,
    pub(crate) mergeable: Mergeable,
    /// How its repository lets it merge, the viewer's default first.
    pub(crate) merge_methods: Vec<MergeMethod>,
    /// The login of the account that read it, whose own posts are not news
    /// to it; `None` when the read didn't say.
    pub(crate) viewer: Option<String>,
    /// Who merged it, once merged.
    pub(crate) merged_by: Option<String>,
    /// The commit its merge made on the base branch, once merged.
    pub(crate) merge_commit: Option<String>,
}

impl PrDetail {
    /// How many checks the read left out, past the first ones it lists:
    /// how they stand is not known.
    pub(crate) fn unread_checks(&self) -> u32 {
        let read = u32::try_from(self.checks.len()).unwrap_or(u32::MAX);
        self.checks_total.saturating_sub(read)
    }
}

/// Whether a PR merges cleanly into its base, as the forge last worked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mergeable {
    Yes,
    Conflicting,
    /// Not worked out yet, or the PR is closed.
    Unknown,
}

/// A way a repository lets a PR merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeMethod {
    /// A merge commit.
    Merge,
    Squash,
    Rebase,
}

/// What someone said on a PR: a comment, a review, or a comment on a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Post {
    /// The forge's own name for it; empty when the read didn't ask.
    pub(crate) id: String,
    pub(crate) author: String,
    /// The author's standing in the repository, as GitHub names it (such as
    /// `OWNER`, `MEMBER` or `NONE`); `None` when the read didn't say.
    pub(crate) author_association: Option<String>,
    /// Where the forge serves the author's picture; `None` for a deleted
    /// account.
    pub(crate) avatar_url: Option<String>,
    /// Unix seconds.
    pub(crate) at: i64,
    /// Markdown; a review may have none.
    pub(crate) body: String,
    /// What a review decided; `None` for a comment.
    pub(crate) verdict: Option<Verdict>,
    /// Where it shows on the forge.
    pub(crate) url: String,
}

/// What a review decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

/// Comments on one place in the code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Thread {
    /// The forge's own name for it.
    pub(crate) id: String,
    pub(crate) path: String,
    /// The line it is on, which an outdated thread no longer has.
    pub(crate) line: Option<u32>,
    pub(crate) state: ThreadState,
    /// The first comments, which start it, and the newest, oldest first
    /// and each once.
    pub(crate) comments: Vec<Post>,
    /// Comments between those, not read.
    pub(crate) more_comments: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThreadState {
    Open,
    /// Open, on code that has changed since.
    Outdated,
    Resolved,
}

/// What one read asks one host: each branch's PR, each linked PR's summary,
/// and the full detail of each PR in `details`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FetchPlan {
    pub(crate) branches: Vec<BranchId>,
    pub(crate) links: Vec<PrId>,
    pub(crate) details: Vec<PrId>,
}

impl FetchPlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.branches.is_empty() && self.links.is_empty() && self.details.is_empty()
    }
}

/// What a forge said to one [`FetchPlan`].
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Batch {
    pub(crate) budget: Option<Budget>,
    /// Each repository's default branch, which has no PR of its own.
    pub(crate) default_branches: Vec<(RepoId, Option<String>)>,
    /// Each branch's PR; a branch of a missing repository isn't here.
    pub(crate) branches: Vec<(BranchId, Found)>,
    /// Each linked PR, [`Found::None`] when there is no such PR.
    pub(crate) links: Vec<(PrId, Found)>,
    pub(crate) details: Vec<(PrId, Option<PrDetail>)>,
    /// Repositories the account can't see, or that don't exist, with what
    /// the forge said.
    pub(crate) missing: Vec<(RepoId, String)>,
}

/// What a lookup found: a branch's PR, or a linked one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    Pr(PrSummary),
    None,
    /// The newest PRs it read, this many, all came from forks' branches of
    /// the same name: the repository's own may be older.
    OnlyForks(usize),
}

/// What a host's API allows its account before its limit resets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    pub(crate) remaining: u32,
    /// Unix seconds.
    pub(crate) resets_at: i64,
    /// What the query that read this cost, in rate-limit points; `None`
    /// when the answer didn't say.
    pub(crate) cost: Option<u32>,
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
