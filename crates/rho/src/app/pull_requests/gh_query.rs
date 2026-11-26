//! Pull requests through `gh api graphql`: one GraphQL query per
//! [`FetchPlan`], built with an alias for each repository (`r0`), branch
//! (`b0`), linked PR (`l0`) and PR read in full (`p0`), and read back by
//! those aliases. A repository the account can't see fails only its own
//! alias; the rest of the batch still answers.

use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::Value;

use super::{
    gh::{classify, Ended, ReadError},
    model::{
        Batch, Budget, ChangeKey, CheckRun, CheckStatus, FetchPlan, Found, MergeMethod, Mergeable,
        Post, PrDetail, PrId, PrState, PrSummary, RepoId, Review, Rollup, Thread, ThreadState,
        Verdict,
    },
};

/// The most summary lookups, branches and links together, that one query
/// asks: a tripwire at the largest batch measured. On 2026-11-22
/// (matthewyjiang/rho, gh 2.99.0), 25 summaries cost 1 point in 1.2 s, and
/// 100 summary aliases still cost 1 point, in 3.0 s (35 KB).
pub(crate) const SUMMARIES_PER_QUERY: usize = 100;

/// The most full details one query asks: a tripwire at the largest batch
/// measured. On 2026-11-22 (matthewyjiang/rho, gh 2.99.0, each detail with
/// `isRequired`), 3 details cost 1 point in 1.6 s, 10 cost 4 points in
/// 2.9 s, and 25 cost 10 points in 4.9 s (812 KB), a twelfth of
/// [`super::gh::DEADLINE`].
pub(crate) const DETAILS_PER_QUERY: usize = 25;

/// `plan` as the plans to send it in, each within [`SUMMARIES_PER_QUERY`]
/// and [`DETAILS_PER_QUERY`]; none for an empty plan. Nothing is dropped:
/// past either limit, the caller sends more than one query, and should log
/// how many.
pub(crate) fn split(plan: FetchPlan) -> Vec<FetchPlan> {
    let FetchPlan {
        branches,
        links,
        details,
    } = plan;
    let mut branches = branches.into_iter();
    let mut links = links.into_iter();
    let mut details = details.into_iter();
    let mut plans = Vec::new();
    loop {
        let branches: Vec<_> = branches.by_ref().take(SUMMARIES_PER_QUERY).collect();
        let room = SUMMARIES_PER_QUERY - branches.len();
        let links: Vec<_> = links.by_ref().take(room).collect();
        let details: Vec<_> = details.by_ref().take(DETAILS_PER_QUERY).collect();
        let plan = FetchPlan {
            branches,
            links,
            details,
        };
        if plan.is_empty() {
            return plans;
        }
        plans.push(plan);
    }
}

/// The PRs a branch's lookup reads, newest first, to find the one from the
/// repository itself: forks' PRs from a branch of the same name, such as
/// `patch-1`, come back too. When all of these are forks', the lookup says
/// so rather than that there is none.
const BRANCH_PRS: usize = 5;

/// What a summary reads of each PR.
const SUMMARY: &str = "fragment Summary on PullRequest {
  number title url state isDraft isCrossRepository updatedAt headRefOid
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
}
";

/// What a full read takes of a PR. rho-ui measured its first 50 checks
/// (far past the 2 rho-ui's CI runs) and its newest 30 comments, 30
/// reviews and 30 threads of 10 comments each at one point a query, on
/// rust-lang/rust#137944 (210 comments, 129 reviews, 84 threads), read in
/// 1.5 s; whether it merges cleanly cost nothing more. Batches of it with
/// `viewer` and `isRequired` were measured on 2026-11-22 (see
/// [`DETAILS_PER_QUERY`]). Comment ids and `publishedAt`, `authorAssociation`, the merge
/// fields and each thread's `latest` five came after those measurements:
/// the poller logs each query's `rateLimit.cost`. How the repository lets
/// it merge cost nothing more (rho-ui measured it on matthewyjiang/rho-ui#35:
/// still 1 point, 0.46 s). A new comment updates the PR's `updatedAt`
/// (cli/cli#10513: comment 20:42:15, update 20:42:16), so a summary read
/// sees it. Each post's author's picture, at 72 pixels (drawn at 24 and
/// sharp to a display scale of 3), cost nothing more either (rho-ui
/// measured it on cli/cli#14578). Neither did the head commit's first 100
/// check suites, which tell a workflow queued before its first job: on
/// 2026-11-26, 25 details of matthewyjiang/rho cost 17 points with them and
/// without, each head having at most 12 suites.
const DETAIL: &str = "fragment Detail on PullRequest {
  ...Summary
  body author { login } headRefName baseRefName additions deletions changedFiles reviewDecision
  mergeable mergedBy { login } mergeCommit { oid }
  repository { mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed viewerDefaultMergeMethod }
  commits(last: 1) { nodes { commit {
    checkSuites(first: 100) { totalCount nodes { id status workflowRun { id } } }
    statusCheckRollup { state contexts(first: 50) { totalCount nodes {
      __typename
      ... on CheckRun {
        name status conclusion detailsUrl checkSuite { id workflowRun { workflow { name } } }
      }
      ... on StatusContext { context state targetUrl }
    } } }
  } } }
  comments(last: 30) { totalCount nodes { ...Post } }
  reviews(last: 30) { totalCount nodes {
    id author { login avatarUrl(size: 72) } authorAssociation body state submittedAt url
  } }
  reviewThreads(last: 30) { totalCount nodes {
    id isResolved isOutdated path line
    comments(first: 10) { totalCount nodes { ...Post } }
    latest: comments(last: 5) { nodes { ...Post } }
  } }
}
fragment Post on Comment {
  id author { login avatarUrl(size: 72) } authorAssociation body createdAt publishedAt
  ... on IssueComment { url } ... on PullRequestReviewComment { url }
}
";

/// Whether each check of PR `number` must pass for it to merge, under the
/// alias `gate`. `isRequired` takes the PR's number, which a fragment
/// can't, so each detail asks with its own. It reads the same contexts as
/// [`DETAIL`], in the same order.
fn gate(number: u32) -> String {
    format!(
        "gate: commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ contexts(first: 50) {{ \
         nodes {{ __typename \
         ... on CheckRun {{ name isRequired(pullRequestNumber: {number}) }} \
         ... on StatusContext {{ context isRequired(pullRequestNumber: {number}) }} \
         }} }} }} }} }} }}"
    )
}

/// The repositories `plan` names, each once, in the order it first names
/// them: their aliases' order.
fn repos(plan: &FetchPlan) -> Vec<&RepoId> {
    let mut repos: Vec<&RepoId> = Vec::new();
    let named = plan
        .branches
        .iter()
        .map(|branch| &branch.repo)
        .chain(plan.links.iter().map(|id| &id.repo))
        .chain(plan.details.iter().map(|id| &id.repo));
    for repo in named {
        if !repos.contains(&repo) {
            repos.push(repo);
        }
    }
    repos
}

/// The query for `plan`. Fragments are only defined where used, as GitHub
/// refuses a query that defines one it doesn't use.
pub(crate) fn query(plan: &FetchPlan) -> String {
    let mut query =
        String::from("query {\n  viewer { login }\n  rateLimit { cost remaining resetAt }\n");
    // Writing to a String can't fail.
    for (ix, repo) in repos(plan).into_iter().enumerate() {
        let _ = writeln!(
            query,
            "  r{ix}: repository(owner: {}, name: {}) {{\n    defaultBranchRef {{ name }}",
            literal(&repo.owner),
            literal(&repo.name)
        );
        for (ix, branch) in plan.branches.iter().enumerate() {
            if branch.repo == *repo {
                let _ = writeln!(
                    query,
                    "    b{ix}: pullRequests(headRefName: {}, first: {BRANCH_PRS}, orderBy: \
                     {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ ...Summary }} }}",
                    literal(&branch.branch)
                );
            }
        }
        for (ix, id) in plan.links.iter().enumerate() {
            if id.repo == *repo {
                let number = id.number;
                let _ = writeln!(
                    query,
                    "    l{ix}: pullRequest(number: {number}) {{ ...Summary }}"
                );
            }
        }
        for (ix, id) in plan.details.iter().enumerate() {
            if id.repo == *repo {
                let number = id.number;
                let gate = gate(number);
                let _ = writeln!(
                    query,
                    "    p{ix}: pullRequest(number: {number}) {{ ...Detail {gate} }}"
                );
            }
        }
        query.push_str("  }\n");
    }
    query.push_str("}\n");
    if !plan.is_empty() {
        query.push_str(SUMMARY);
    }
    if !plan.details.is_empty() {
        query.push_str(DETAIL);
    }
    query
}

/// `text` as a GraphQL string: JSON's escapes are GraphQL's.
fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// What `gh` answered to `plan`'s query: its output `stdout` and its error
/// output `stderr`.
///
/// # Errors
///
/// When `gh` answered nothing usable: why, and whether its account is
/// signed out.
pub(crate) fn parse(plan: &FetchPlan, stdout: &[u8], stderr: &str) -> Result<Batch, ReadError> {
    let stderr = stderr.trim();
    let response: Value = match serde_json::from_slice(stdout) {
        Ok(response) => response,
        Err(_) if !stderr.is_empty() => return Err(classify(Ended::Said(stderr))),
        Err(why) => return Err(ReadError::Failed(format!("gh's answer isn't JSON: {why}"))),
    };
    let Some(data) = response.get("data").filter(|data| data.is_object()) else {
        let messages: Vec<&str> = response["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|error| error["message"].as_str())
            .collect();
        let why = if messages.is_empty() {
            stderr.to_owned()
        } else {
            messages.join(" ")
        };
        if why.is_empty() {
            return Err(ReadError::Failed(
                "GitHub answered with neither data nor errors".to_owned(),
            ));
        }
        return Err(classify(Ended::Said(&why)));
    };
    let errors = response["errors"].as_array().map_or(&[][..], Vec::as_slice);
    batch(plan, data, errors)
}

/// The answer to `plan` in `data`, by its aliases. A repository the
/// account can't see, as `errors` says of its alias, is missing; a PR
/// number with no PR reads as none; a check whose `gate` failed reads as
/// not saying whether it is required; any other error fails the whole
/// answer, as does one that doesn't read.
fn batch(plan: &FetchPlan, data: &Value, errors: &[Value]) -> Result<Batch, ReadError> {
    let repos = repos(plan);
    let mut batch = Batch {
        budget: budget(&data["rateLimit"]),
        ..Batch::default()
    };
    let viewer = data["viewer"]["login"].as_str();
    for error in errors {
        let message = error["message"]
            .as_str()
            .unwrap_or("GitHub refused the query.");
        let alias = match error["path"].as_array().map(Vec::as_slice) {
            Some([Value::String(alias)]) => alias.as_str(),
            // GitHub says so of a PR number with no PR, such as a typo in a
            // link, and answers its alias with null.
            Some([Value::String(_), Value::String(pr)])
                if error["type"] == "NOT_FOUND" && pr.starts_with(['l', 'p']) =>
            {
                continue;
            }
            // GitHub answers the check's `gate` node with null.
            Some([Value::String(_), Value::String(pr), Value::String(field), ..])
                if pr.starts_with('p') && field == "gate" =>
            {
                continue;
            }
            _ => return Err(ReadError::Failed(message.to_owned())),
        };
        let repo = alias
            .strip_prefix('r')
            .and_then(|ix| ix.parse::<usize>().ok())
            .and_then(|ix| repos.get(ix));
        match (repo, error["type"].as_str()) {
            (Some(repo), Some("NOT_FOUND" | "FORBIDDEN")) => {
                batch.missing.push(((*repo).clone(), message.to_owned()));
            }
            _ => return Err(ReadError::Failed(message.to_owned())),
        }
    }
    let unreadable =
        |why: serde_json::Error| ReadError::Failed(format!("GitHub's answer didn't read: {why}"));
    for (ix, repo) in repos.into_iter().enumerate() {
        if batch.missing.iter().any(|(missing, _)| missing == repo) {
            continue;
        }
        let answer = &data[format!("r{ix}")];
        let default = answer["defaultBranchRef"]["name"]
            .as_str()
            .map(str::to_owned);
        batch.default_branches.push((repo.clone(), default));
        for (ix, branch) in plan.branches.iter().enumerate() {
            if branch.repo != *repo {
                continue;
            }
            let nodes = answer[format!("b{ix}")]["nodes"].clone();
            let found: Vec<GhSummary> = serde_json::from_value(nodes).map_err(unreadable)?;
            batch.branches.push((branch.clone(), pick(repo, found)));
        }
        for (ix, id) in plan.links.iter().enumerate() {
            if id.repo != *repo {
                continue;
            }
            // A linked PR may be a fork's.
            let read: Option<GhSummary> =
                serde_json::from_value(answer[format!("l{ix}")].clone()).map_err(unreadable)?;
            let found = read.map_or(Found::None, |read| Found::Pr(read.into_summary(repo)));
            batch.links.push((id.clone(), found));
        }
        for (ix, id) in plan.details.iter().enumerate() {
            if id.repo != *repo {
                continue;
            }
            let read: Option<GhDetail> =
                serde_json::from_value(answer[format!("p{ix}")].clone()).map_err(unreadable)?;
            let detail = read.map(|read| read.into_detail(repo, viewer));
            batch.details.push((id.clone(), detail));
        }
    }
    Ok(batch)
}

fn budget(rate_limit: &Value) -> Option<Budget> {
    let remaining = u32::try_from(rate_limit["remaining"].as_u64()?).ok()?;
    let resets_at = unix_seconds(rate_limit["resetAt"].as_str()?)?;
    let cost = rate_limit["cost"]
        .as_u64()
        .and_then(|cost| u32::try_from(cost).ok());
    Some(Budget {
        remaining,
        resets_at,
        cost,
    })
}

/// Unix seconds from GitHub's `2026-11-15T06:18:47Z`.
fn unix_seconds(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp())
}

/// A branch's PR among `found`, newest first: one from the repository
/// itself, not a fork's of the same name, preferring one still open.
fn pick(repo: &RepoId, found: Vec<GhSummary>) -> Found {
    let read = found.len();
    let own: Vec<GhSummary> = found
        .into_iter()
        .filter(|pr| !pr.is_cross_repository)
        .collect();
    let open = own.iter().position(|pr| pr.state == "OPEN").unwrap_or(0);
    match own.into_iter().nth(open) {
        Some(pr) => Found::Pr(pr.into_summary(repo)),
        None if read == BRANCH_PRS => Found::OnlyForks(BRANCH_PRS),
        None => Found::None,
    }
}

/// A PR as a summary reads it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhSummary {
    number: u32,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    #[serde(default)]
    is_cross_repository: bool,
    updated_at: String,
    head_ref_oid: String,
    commits: Nodes<CommitNode>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct CommitNode {
    commit: Commit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    status_check_rollup: Option<GhRollup>,
    /// Asked by a full read only.
    #[serde(default)]
    check_suites: Option<Connection<GhSuite>>,
}

/// A check suite of the head commit: one app's checks on it, such as one
/// GitHub Actions workflow run's jobs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhSuite {
    id: String,
    status: String,
    /// Set for a GitHub Actions workflow run's suite only.
    workflow_run: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct GhRollup {
    state: String,
    #[serde(default)]
    contexts: Option<Connection<GhCheck>>,
}

/// A check: a CI job, or a status another service set on the commit.
#[derive(Deserialize)]
#[serde(tag = "__typename", rename_all_fields = "camelCase")]
enum GhCheck {
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        details_url: Option<String>,
        check_suite: Option<CheckSuite>,
    },
    StatusContext {
        context: String,
        state: String,
        target_url: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckSuite {
    #[serde(default)]
    id: Option<String>,
    workflow_run: Option<WorkflowRun>,
}

#[derive(Deserialize)]
struct WorkflowRun {
    workflow: Workflow,
}

#[derive(Deserialize)]
struct Workflow {
    name: String,
}

/// The `gate` alias's head commit: whether each check is required.
#[derive(Deserialize)]
struct GateNode {
    commit: GateCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GateCommit {
    status_check_rollup: Option<GateRollup>,
}

#[derive(Deserialize)]
struct GateRollup {
    /// `null` where GitHub couldn't say.
    contexts: Nodes<Option<GhGate>>,
}

/// Whether one check is required, named so it can be matched to its check.
#[derive(Deserialize)]
#[serde(tag = "__typename", rename_all_fields = "camelCase")]
enum GhGate {
    CheckRun { name: String, is_required: bool },
    StatusContext { context: String, is_required: bool },
}

impl GhGate {
    /// Whether the check named `check` is required, if this is its gate.
    fn required_for(self, check: &str) -> Option<bool> {
        match self {
            GhGate::CheckRun { name, is_required }
            | GhGate::StatusContext {
                context: name,
                is_required,
            } => (name == check).then_some(is_required),
        }
    }
}

/// A PR as a full read takes it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhDetail {
    #[serde(flatten)]
    summary: GhSummary,
    body: String,
    author: Option<Login>,
    head_ref_name: String,
    base_ref_name: String,
    additions: u32,
    deletions: u32,
    changed_files: u32,
    review_decision: Option<String>,
    mergeable: String,
    repository: GhMergeSettings,
    #[serde(default)]
    merged_by: Option<Login>,
    #[serde(default)]
    merge_commit: Option<Oid>,
    comments: Connection<GhComment>,
    reviews: Connection<GhReview>,
    review_threads: Connection<GhThread>,
    #[serde(default)]
    gate: Option<Nodes<GateNode>>,
}

/// How a PR's repository lets PRs merge.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhMergeSettings {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    viewer_default_merge_method: Option<String>,
}

impl GhMergeSettings {
    /// The methods allowed, the viewer's default first, else in GitHub's
    /// order.
    fn methods(&self) -> Vec<MergeMethod> {
        let default = match self.viewer_default_merge_method.as_deref() {
            Some("SQUASH") => Some(MergeMethod::Squash),
            Some("REBASE") => Some(MergeMethod::Rebase),
            Some("MERGE") => Some(MergeMethod::Merge),
            _ => None,
        };
        let mut methods: Vec<MergeMethod> = [
            (MergeMethod::Merge, self.merge_commit_allowed),
            (MergeMethod::Squash, self.squash_merge_allowed),
            (MergeMethod::Rebase, self.rebase_merge_allowed),
        ]
        .into_iter()
        .filter_map(|(method, allowed)| allowed.then_some(method))
        .collect();
        // Stable: the default first, the rest in GitHub's order.
        methods.sort_by_key(|method| Some(*method) != default);
        methods
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Login {
    login: String,
    /// Asked for posts' authors only.
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct Oid {
    oid: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection<T> {
    total_count: u32,
    nodes: Vec<T>,
}

impl<T> Connection<T> {
    /// How many it has past those read.
    fn unread(&self) -> u32 {
        let read = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        self.total_count.saturating_sub(read)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhComment {
    #[serde(default)]
    id: String,
    author: Option<Login>,
    #[serde(default)]
    author_association: Option<String>,
    body: String,
    created_at: String,
    /// When it was made public, which for a comment of a review written
    /// over time is when the review was sent; `None` for a draft, or when
    /// the read didn't ask.
    #[serde(default)]
    published_at: Option<String>,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReview {
    #[serde(default)]
    id: String,
    author: Option<Login>,
    #[serde(default)]
    author_association: Option<String>,
    body: String,
    state: String,
    submitted_at: Option<String>,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhThread {
    id: String,
    is_resolved: bool,
    is_outdated: bool,
    path: String,
    line: Option<u32>,
    comments: Connection<GhComment>,
    /// Its newest comments, which `comments` may not reach.
    #[serde(default)]
    latest: Option<Nodes<GhComment>>,
}

/// Whose it is, and their picture: a deleted account's reads as GitHub
/// shows it, `ghost`, with none.
fn login(author: Option<Login>) -> (String, Option<String>) {
    author.map_or_else(
        || ("ghost".to_owned(), None),
        |author| (author.login, author.avatar_url),
    )
}

impl GhComment {
    fn into_post(self) -> Post {
        let (author, avatar_url) = login(self.author);
        Post {
            id: self.id,
            author,
            author_association: self.author_association,
            avatar_url,
            at: unix_seconds(self.published_at.as_deref().unwrap_or(&self.created_at))
                .unwrap_or_default(),
            body: self.body,
            verdict: None,
            url: self.url,
        }
    }
}

impl GhReview {
    /// The review as a post, unless it says nothing: a review that only
    /// holds comments on lines, which their threads show, or one still
    /// being written.
    fn into_post(self) -> Option<Post> {
        let verdict = match self.state.as_str() {
            "APPROVED" => Verdict::Approved,
            "CHANGES_REQUESTED" => Verdict::ChangesRequested,
            "DISMISSED" => Verdict::Dismissed,
            "COMMENTED" if !self.body.trim().is_empty() => Verdict::Commented,
            _ => return None,
        };
        let (author, avatar_url) = login(self.author);
        Some(Post {
            id: self.id,
            author,
            author_association: self.author_association,
            avatar_url,
            at: unix_seconds(self.submitted_at.as_deref()?).unwrap_or_default(),
            body: self.body,
            verdict: Some(verdict),
            url: self.url,
        })
    }
}

/// The newest comments and reviews, oldest first, and how many older ones
/// weren't read. Of two lists cut short, each at its newest, the posts
/// older than the newer cut go too: the other list's posts from then
/// weren't read, and the timeline would have a hole.
fn timeline(comments: Connection<GhComment>, reviews: Connection<GhReview>) -> (Vec<Post>, u32) {
    let mut older = comments.unread() + reviews.unread();
    let oldest = |posts: &[Post], unread: u32| {
        (unread > 0)
            .then(|| posts.iter().map(|post| post.at).min())
            .flatten()
    };
    let comments_unread = comments.unread();
    let reviews_unread = reviews.unread();
    let comments: Vec<Post> = comments
        .nodes
        .into_iter()
        .map(GhComment::into_post)
        .collect();
    let reviews: Vec<Post> = reviews
        .nodes
        .into_iter()
        .filter_map(GhReview::into_post)
        .collect();
    let cut = oldest(&comments, comments_unread).max(oldest(&reviews, reviews_unread));
    let mut posts: Vec<Post> = comments.into_iter().chain(reviews).collect();
    if let Some(cut) = cut {
        let before = posts.len();
        posts.retain(|post| post.at >= cut);
        older += u32::try_from(before - posts.len()).unwrap_or(u32::MAX);
    }
    posts.sort_by_key(|post| post.at);
    (posts, older)
}

impl GhThread {
    /// The thread with its first comments and its newest, each once.
    fn into_thread(self) -> Thread {
        let total = self.comments.total_count;
        let mut comments: Vec<Post> = self
            .comments
            .nodes
            .into_iter()
            .map(GhComment::into_post)
            .collect();
        let latest = self.latest.into_iter().flat_map(|latest| latest.nodes);
        for post in latest.map(GhComment::into_post) {
            if !comments.iter().any(|read| read.url == post.url) {
                comments.push(post);
            }
        }
        comments.sort_by_key(|post| post.at);
        let read = u32::try_from(comments.len()).unwrap_or(u32::MAX);
        Thread {
            id: self.id,
            path: self.path,
            line: self.line,
            state: match (self.is_resolved, self.is_outdated) {
                (true, _) => ThreadState::Resolved,
                (false, true) => ThreadState::Outdated,
                (false, false) => ThreadState::Open,
            },
            comments,
            more_comments: total.saturating_sub(read),
        }
    }
}

impl GhSummary {
    fn rollup(&self) -> Option<&GhRollup> {
        self.commits
            .nodes
            .last()?
            .commit
            .status_check_rollup
            .as_ref()
    }

    fn into_summary(self, repo: &RepoId) -> PrSummary {
        let rollup = match self.rollup().map(|rollup| rollup.state.as_str()) {
            None => Rollup::None,
            Some("SUCCESS") => Rollup::Passing,
            Some("FAILURE" | "ERROR") => Rollup::Failing,
            Some(_) => Rollup::Pending,
        };
        let state = match (self.state.as_str(), self.is_draft) {
            ("MERGED", _) => PrState::Merged,
            ("CLOSED", _) => PrState::Closed,
            (_, true) => PrState::Draft,
            _ => PrState::Open,
        };
        PrSummary {
            id: PrId {
                repo: repo.clone(),
                number: self.number,
            },
            title: self.title,
            url: self.url,
            state,
            change: ChangeKey {
                updated_at: self.updated_at,
                head_oid: self.head_ref_oid,
                rollup,
            },
        }
    }
}

impl GhDetail {
    /// The detail as `viewer` read it.
    fn into_detail(mut self, repo: &RepoId, viewer: Option<&str>) -> PrDetail {
        let commit = self
            .summary
            .commits
            .nodes
            .last_mut()
            .map(|node| &mut node.commit);
        let (contexts, suites) = commit.map_or((None, None), |commit| {
            let contexts = commit
                .status_check_rollup
                .as_mut()
                .and_then(|rollup| rollup.contexts.take());
            (contexts, commit.check_suites.take())
        });
        let queued_workflows = suites.map_or(0, |suites| {
            let checks = contexts
                .as_ref()
                .map_or(&[][..], |contexts| &contexts.nodes);
            queued_workflows(suites, checks)
        });
        let mut gates = self
            .gate
            .take()
            .and_then(|gate| gate.nodes.into_iter().last())
            .and_then(|node| node.commit.status_check_rollup)
            .map(|rollup| rollup.contexts.nodes)
            .unwrap_or_default()
            .into_iter();
        let (mut checks, checks_total) = match contexts {
            Some(contexts) => (
                contexts
                    .nodes
                    .into_iter()
                    .map(|check| {
                        let mut check = check.into_check();
                        check.required = gates
                            .next()
                            .flatten()
                            .and_then(|gate| gate.required_for(&check.name));
                        check
                    })
                    .collect(),
                contexts.total_count,
            ),
            None => (Vec::new(), 0),
        };
        checks.sort_by_key(|check: &CheckRun| check.status);
        let review = match self.review_decision.as_deref() {
            Some("APPROVED") => Review::Approved,
            Some("CHANGES_REQUESTED") => Review::ChangesRequested,
            Some("REVIEW_REQUIRED") => Review::Required,
            _ => Review::None,
        };
        let (posts, older_posts) = timeline(self.comments, self.reviews);
        let mergeable = match self.mergeable.as_str() {
            "MERGEABLE" => Mergeable::Yes,
            "CONFLICTING" => Mergeable::Conflicting,
            _ => Mergeable::Unknown,
        };
        let older_threads = self.review_threads.unread();
        PrDetail {
            posts,
            older_posts,
            threads: self
                .review_threads
                .nodes
                .into_iter()
                .map(GhThread::into_thread)
                .collect(),
            older_threads,
            summary: self.summary.into_summary(repo),
            author: self.author.map(|author| author.login).unwrap_or_default(),
            head: self.head_ref_name,
            base: self.base_ref_name,
            body: self.body,
            additions: self.additions,
            deletions: self.deletions,
            changed_files: self.changed_files,
            review,
            checks,
            checks_total,
            queued_workflows,
            mergeable,
            merge_methods: self.repository.methods(),
            viewer: viewer.map(str::to_owned),
            merged_by: self.merged_by.map(|author| author.login),
            merge_commit: self.merge_commit.map(|commit| commit.oid),
        }
    }
}

/// How many of the head commit's GitHub Actions workflow runs are still
/// going with none of their jobs listed as queued or running: queued before
/// their first job, or between one job and those that need it. GitHub lists
/// a job as a check only once it is made, so each such run has checks still
/// to come that `checks` can't show. Another app's suite doesn't count:
/// GitHub makes one for every installed app that listens for checks, and
/// one whose app never reports stays `QUEUED` with no checks for good.
/// Suites past those read count too, as any of them may be such a run.
fn queued_workflows(suites: Connection<GhSuite>, checks: &[GhCheck]) -> u32 {
    let busy: Vec<&str> = checks
        .iter()
        .filter_map(|check| match check {
            GhCheck::CheckRun {
                status,
                check_suite: Some(CheckSuite { id: Some(id), .. }),
                ..
            } if status != "COMPLETED" => Some(id.as_str()),
            GhCheck::CheckRun { .. } | GhCheck::StatusContext { .. } => None,
        })
        .collect();
    let unread = suites.unread();
    let queued = suites
        .nodes
        .iter()
        .filter(|suite| {
            suite.workflow_run.is_some()
                && suite.status != "COMPLETED"
                && !busy.contains(&suite.id.as_str())
        })
        .count();
    u32::try_from(queued)
        .unwrap_or(u32::MAX)
        .saturating_add(unread)
}

impl GhCheck {
    /// The check, not yet knowing whether it is required.
    fn into_check(self) -> CheckRun {
        match self {
            GhCheck::CheckRun {
                name,
                status,
                conclusion,
                details_url,
                check_suite,
            } => {
                let status = match (status.as_str(), conclusion.as_deref()) {
                    ("COMPLETED", Some("SUCCESS")) => CheckStatus::Passed,
                    ("COMPLETED", Some("CANCELLED")) => CheckStatus::Cancelled,
                    ("COMPLETED", Some("SKIPPED")) => CheckStatus::Skipped,
                    ("COMPLETED", Some("NEUTRAL" | "STALE")) => CheckStatus::Neutral,
                    ("COMPLETED", _) => CheckStatus::Failed,
                    ("IN_PROGRESS", _) => CheckStatus::Running,
                    _ => CheckStatus::Pending,
                };
                let workflow = check_suite
                    .and_then(|suite| suite.workflow_run)
                    .map(|run| run.workflow.name);
                CheckRun {
                    name,
                    workflow,
                    status,
                    url: details_url,
                    required: None,
                }
            }
            GhCheck::StatusContext {
                context,
                state,
                target_url,
            } => CheckRun {
                name: context,
                workflow: None,
                status: match state.as_str() {
                    "SUCCESS" => CheckStatus::Passed,
                    "FAILURE" | "ERROR" => CheckStatus::Failed,
                    _ => CheckStatus::Pending,
                },
                url: target_url,
                required: None,
            },
        }
    }
}

#[cfg(test)]
#[path = "gh_query_tests.rs"]
mod tests;
