use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::*;
use crate::app::pull_requests::model::BranchId;

/// 2026-11-22T00:00:00Z.
const MIDNIGHT: i64 = 1_795_305_600;

fn repo(name: &str) -> RepoId {
    RepoId {
        host: "github.com".to_owned(),
        owner: "matthewyjiang".to_owned(),
        name: name.to_owned(),
    }
}

fn branch(name: &str, branch: &str) -> BranchId {
    BranchId {
        repo: repo(name),
        branch: branch.to_owned(),
    }
}

fn pr(name: &str, number: u32) -> PrId {
    PrId {
        repo: repo(name),
        number,
    }
}

/// The plan the fixture answers: two branches and a PR of rho-ui, and a
/// branch of a repository that doesn't exist. The fixture is a real answer
/// to rho-ui's query for it, its PR's body cut short.
fn fixture_plan() -> FetchPlan {
    FetchPlan {
        branches: vec![
            branch("rho-ui", "ci-and-pr-template"),
            branch("rho-ui", "no-such-branch"),
            branch("does-not-exist-xyz", "main"),
        ],
        links: Vec::new(),
        details: vec![pr("rho-ui", 32)],
    }
}

// Covers: the query names each repository once, escapes what it quotes,
// defines only the fragments it spreads (GitHub refuses one defined and
// unused), asks for the budget's cost, and asks each detail's gate with its
// own PR number.
// Owner: pure unit (wire contract)
#[test]
fn the_query_aliases_each_repository_once() {
    let text = query(&fixture_plan());
    assert_eq!(text.matches("repository(").count(), 2);
    assert!(
        text.contains("rateLimit { cost remaining resetAt }"),
        "{text}"
    );

    let lookups = FetchPlan {
        branches: vec![branch("rho-ui", "say-\"hi\"")],
        links: Vec::new(),
        details: Vec::new(),
    };
    let text = query(&lookups);
    assert!(text.contains(r#"headRefName: "say-\"hi\"""#), "{text}");
    assert!(!text.contains("fragment Detail"), "defined but unused");

    let details = FetchPlan {
        branches: Vec::new(),
        links: Vec::new(),
        details: vec![pr("rho-ui", 32), pr("rho", 7)],
    };
    let text = query(&details);
    let gated: Vec<(u32, usize)> = text
        .lines()
        .filter_map(|line| {
            let rest = line.split_once("pullRequest(number: ")?.1;
            let number: u32 = rest.split(')').next()?.parse().ok()?;
            let gates = format!("isRequired(pullRequestNumber: {number})");
            Some((number, line.matches(&gates).count()))
        })
        .collect();
    assert_eq!(gated, [(32, 2), (7, 2)], "{text}");
}

// Covers: a real answer reads by its aliases: a branch's own PR with its
// checks passed, a branch without one, a PR's detail with its checks, and a
// repository that isn't there, which fails only its own alias.
// Owner: pure unit (wire contract)
#[test]
fn a_real_answer_reads_by_its_aliases() {
    let plan = fixture_plan();
    let answer = include_str!("fixtures/gh_batch.json");
    let error =
        "gh: Could not resolve to a Repository with the name 'matthewyjiang/does-not-exist-xyz'.";
    let batch = parse(&plan, answer.as_bytes(), error).unwrap();
    assert_eq!(
        batch.budget,
        Some(Budget {
            remaining: 4988,
            // 2026-11-15T06:18:47Z
            resets_at: 1_794_723_527,
            cost: None,
        })
    );
    assert_eq!(
        batch.missing,
        [(
            repo("does-not-exist-xyz"),
            "Could not resolve to a Repository with the name \
             'matthewyjiang/does-not-exist-xyz'."
                .to_owned()
        )]
    );
    assert_eq!(
        batch.default_branches,
        [(repo("rho-ui"), Some("main".to_owned()))]
    );
    let [(_, Found::Pr(found)), (_, Found::None)] = batch.branches.as_slice() else {
        panic!("one PR and no PR: {:?}", batch.branches);
    };
    assert_eq!(
        (found.id.number, found.state, found.change.rollup),
        (32, PrState::Merged, Rollup::Passing)
    );
    let [(_, Some(detail))] = batch.details.as_slice() else {
        panic!("PR 32's detail: {:?}", batch.details);
    };
    assert_eq!(detail.summary, *found, "the detail reads the same summary");
    assert_eq!(
        (detail.additions, detail.deletions, detail.changed_files),
        (111, 4, 8)
    );
    assert_eq!(detail.checks_total, 2);
    let checks: Vec<(CheckStatus, Option<&str>)> = detail
        .checks
        .iter()
        .map(|check| (check.status, check.workflow.as_deref()))
        .collect();
    assert_eq!(
        checks,
        [
            (CheckStatus::Passed, Some("CI")),
            (CheckStatus::Passed, Some("CI"))
        ]
    );
}

// Covers: a link to a PR number with no PR reads as no PR, in GitHub's real
// words for it, and leaves the rest of the answer alone.
// Owner: pure unit (wire contract)
#[test]
fn a_link_to_no_pr_finds_none() {
    let plan = FetchPlan {
        branches: vec![branch("rho-ui", "feature")],
        links: vec![pr("rho-ui", 999_999_999)],
        details: Vec::new(),
    };
    let answer = r#"{"data":{"rateLimit":{"remaining":4990,"resetAt":"2026-11-15T06:18:47Z"},
        "r0":{"defaultBranchRef":{"name":"main"},"b0":{"nodes":[]},"l0":null}},
        "errors":[{"type":"NOT_FOUND","path":["r0","l0"],"locations":[{"line":4,"column":5}],
        "message":"Could not resolve to a PullRequest with the number of 999999999."}]}"#;
    let batch = parse(&plan, answer.as_bytes(), "").unwrap();
    assert_eq!(batch.links, [(pr("rho-ui", 999_999_999), Found::None)]);
    assert_eq!(batch.branches, [(branch("rho-ui", "feature"), Found::None)]);
    assert!(batch.missing.is_empty(), "{:?}", batch.missing);
}

// Covers: a real conversation, cli/cli#14578, bodies cut short: its comments
// and reviews in one timeline, oldest first, without the reviews that only
// hold comments on lines; and its threads, resolved, on lines since changed
// or gone.
// Owner: pure unit (wire contract)
#[test]
fn a_real_conversation_reads_oldest_first() {
    let id = PrId {
        repo: RepoId {
            host: "github.com".to_owned(),
            owner: "cli".to_owned(),
            name: "cli".to_owned(),
        },
        number: 14578,
    };
    let plan = FetchPlan {
        branches: Vec::new(),
        links: Vec::new(),
        details: vec![id],
    };
    let answer = include_str!("fixtures/gh_conversation.json");
    let batch = parse(&plan, answer.as_bytes(), "").unwrap();
    let [(_, Some(detail))] = batch.details.as_slice() else {
        panic!("the PR's detail: {:?}", batch.details);
    };
    let said: Vec<(&str, Option<Verdict>)> = detail
        .posts
        .iter()
        .map(|post| (post.author.as_str(), post.verdict))
        .collect();
    assert_eq!(
        said,
        [
            ("copilot-pull-request-reviewer", Some(Verdict::Commented)),
            ("BagToad", None),
            ("copilot-pull-request-reviewer", Some(Verdict::Commented)),
            ("BagToad", None),
            ("williammartin", None),
            ("BagToad", None),
            ("BagToad", Some(Verdict::Approved)),
            ("williammartin", None),
        ]
    );
    assert!(detail.posts.is_sorted_by_key(|post| post.at));
    assert_eq!(detail.older_posts, 0);
    let threads: Vec<(ThreadState, Option<u32>, usize)> = detail
        .threads
        .iter()
        .map(|thread| (thread.state, thread.line, thread.comments.len()))
        .collect();
    assert_eq!(threads.len(), 5);
    assert!(threads
        .iter()
        .all(|(state, _, comments)| *state == ThreadState::Resolved && *comments == 2));
    assert_eq!(threads[0].1, None, "outdated past its line");
    assert_eq!(threads[1].1, Some(423));
    assert_eq!(
        detail.merge_methods,
        [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]
    );
}

// Covers: a PR offers the merge methods its repository allows, the
// viewer's default first, else in GitHub's order, so a merge menu never
// offers one the repository forbids.
// Owner: pure unit (parser)
#[test]
fn merges_offer_what_the_repository_allows() {
    use MergeMethod::{Merge, Rebase, Squash};
    let settings = |allowed: [bool; 3], default: Option<&str>| GhMergeSettings {
        merge_commit_allowed: allowed[0],
        squash_merge_allowed: allowed[1],
        rebase_merge_allowed: allowed[2],
        viewer_default_merge_method: default.map(str::to_owned),
    };
    let cases = [
        ([true; 3], Some("REBASE"), vec![Rebase, Merge, Squash]),
        ([true; 3], None, vec![Merge, Squash, Rebase]),
        ([false, true, false], Some("MERGE"), vec![Squash]),
    ];
    for (allowed, default, expected) in cases {
        assert_eq!(
            settings(allowed, default).methods(),
            expected,
            "{allowed:?} {default:?}"
        );
    }
}

// Covers: comments and reviews cut short at their newest: posts older than
// the later cut go, as the other list's posts from then weren't read, and
// count as older.
// Owner: pure unit (parser)
#[test]
fn a_cut_timeline_has_no_holes() {
    let comment = |at: &str| GhComment {
        id: String::new(),
        author: None,
        author_association: None,
        body: "c".to_owned(),
        created_at: at.to_owned(),
        published_at: None,
        url: String::new(),
    };
    let review = |at: &str| GhReview {
        id: String::new(),
        author: None,
        author_association: None,
        body: String::new(),
        state: "APPROVED".to_owned(),
        submitted_at: Some(at.to_owned()),
        url: String::new(),
    };
    let comments = Connection {
        total_count: 10,
        nodes: vec![
            comment("2026-01-02T00:00:00Z"),
            comment("2026-01-04T00:00:00Z"),
        ],
    };
    let reviews = Connection {
        total_count: 2,
        nodes: vec![
            review("2026-01-01T00:00:00Z"),
            review("2026-01-03T00:00:00Z"),
        ],
    };
    let (posts, older) = timeline(comments, reviews);
    let verdicts: Vec<Option<Verdict>> = posts.iter().map(|post| post.verdict).collect();
    assert_eq!(
        verdicts,
        [None, Some(Verdict::Approved), None],
        "the cut is the oldest comment read"
    );
    assert_eq!(older, 8 + 1);
}

// Covers: a branch named like a fork's takes the repository's own PR, an
// open one over an older closed one; with only forks' PRs, it has none,
// unless they fill the lookup, which may hide its own.
// Owner: pure unit (parser)
#[test]
fn a_branch_takes_its_own_open_pr() {
    #[derive(Debug, PartialEq)]
    enum Picked {
        Pr(u32),
        None,
        OnlyForks(usize),
    }
    let pr = |number: u32, state: &str, fork: bool| {
        json!({
            "number": number, "title": "t", "url": "u", "state": state, "isDraft": false,
            "isCrossRepository": fork, "updatedAt": "2026-11-15T00:00:00Z",
            "headRefOid": "abc", "commits": { "nodes": [] }
        })
    };
    let picked = |nodes: Value| {
        let found: Vec<GhSummary> = serde_json::from_value(nodes).unwrap();
        match pick(&repo("rho-ui"), found) {
            Found::Pr(pr) => Picked::Pr(pr.id.number),
            Found::None => Picked::None,
            Found::OnlyForks(read) => Picked::OnlyForks(read),
        }
    };
    let forks: Vec<Value> = (1..=5).map(|number| pr(number, "OPEN", true)).collect();
    let cases = [
        (
            json!([pr(9, "OPEN", true), pr(7, "CLOSED", false)]),
            Picked::Pr(7),
        ),
        (
            json!([pr(8, "MERGED", false), pr(5, "OPEN", false)]),
            Picked::Pr(5),
        ),
        (json!([pr(9, "OPEN", true)]), Picked::None),
        (json!([]), Picked::None),
        (Value::Array(forks), Picked::OnlyForks(BRANCH_PRS)),
    ];
    for (nodes, expected) in cases {
        assert_eq!(picked(nodes.clone()), expected, "{nodes}");
    }
}

// Covers: what the watch needs from a full read: whether each check is
// required (from the `gate` alias, a gate GitHub failed reading as unknown
// rather than failing the read), the viewer, post ids and author
// associations, a post's time as published (a comment of a review sent
// later) else as created, a thread's newest comments past its first, who
// merged it and as what commit, how its repository lets it merge, and the
// query's cost. The answer mirrors GitHub's shape for the query `query`
// builds.
// Owner: pure unit (wire contract)
#[test]
fn a_full_read_takes_what_the_watch_needs() {
    let widgets = RepoId {
        host: "github.com".to_owned(),
        owner: "acme".to_owned(),
        name: "widgets".to_owned(),
    };
    let id = PrId {
        repo: widgets,
        number: 12,
    };
    let plan = FetchPlan {
        branches: Vec::new(),
        links: Vec::new(),
        details: vec![id.clone()],
    };
    let url = "https://github.com/acme/widgets/pull/12";
    // Comment `n` of the thread, written at midnight in a review sent at
    // 01:`n`: it was posted when the review was.
    let thread_comment = |n: i64| {
        json!({
            "id": format!("PRRC_{n}"), "author": { "login": "carol" },
            "authorAssociation": "CONTRIBUTOR", "body": format!("reply {n}"),
            "createdAt": "2026-11-22T00:00:00Z",
            "publishedAt": format!("2026-11-22T01:{n:02}:00Z"),
            "url": format!("{url}#discussion_r{n}")
        })
    };
    let check_run = |name: &str, job: u32| {
        json!({
            "__typename": "CheckRun", "name": name, "status": "COMPLETED",
            "conclusion": "SUCCESS",
            "detailsUrl": format!("https://github.com/acme/widgets/actions/runs/1/job/{job}"),
            "checkSuite": { "workflowRun": { "workflow": { "name": "CI" } } }
        })
    };
    let answer = json!({
        "data": {
            "viewer": { "login": "rho-bot" },
            "rateLimit": { "cost": 1, "remaining": 4990, "resetAt": "2026-11-22T07:00:00Z" },
            "r0": {
                "defaultBranchRef": { "name": "main" },
                "p0": {
                    "number": 12, "title": "Add widgets", "url": url, "state": "MERGED",
                    "isDraft": false, "isCrossRepository": false,
                    "updatedAt": "2026-11-22T06:00:00Z", "headRefOid": "1a2b3c4",
                    "commits": { "nodes": [{ "commit": { "statusCheckRollup": {
                        "state": "SUCCESS",
                        "contexts": { "totalCount": 3, "nodes": [
                            check_run("test", 2),
                            check_run("lint", 3),
                            { "__typename": "StatusContext", "context": "deploy/preview",
                              "state": "SUCCESS", "targetUrl": null }
                        ] }
                    } } }] },
                    "body": "", "author": { "login": "alice" }, "headRefName": "widgets",
                    "baseRefName": "main", "additions": 1, "deletions": 0, "changedFiles": 1,
                    "reviewDecision": "APPROVED", "mergeable": "UNKNOWN",
                    "mergedBy": { "login": "bob" }, "mergeCommit": { "oid": "9f8e7d6" },
                    "repository": {
                        "mergeCommitAllowed": false, "squashMergeAllowed": true,
                        "rebaseMergeAllowed": true, "viewerDefaultMergeMethod": "REBASE"
                    },
                    "comments": { "totalCount": 1, "nodes": [{
                        "id": "IC_1", "author": { "login": "carol" },
                        "authorAssociation": "NONE", "body": "nice",
                        "createdAt": "2026-11-22T00:30:00Z",
                        "url": format!("{url}#issuecomment-1")
                    }] },
                    "reviews": { "totalCount": 1, "nodes": [{
                        "id": "PRR_1",
                        "author": { "login": "bob", "avatarUrl": "https://avatars.githubusercontent.com/u/2?s=72" },
                        "authorAssociation": "OWNER", "body": "", "state": "APPROVED",
                        "submittedAt": "2026-11-22T00:45:00Z",
                        "url": format!("{url}#pullrequestreview-1")
                    }] },
                    "reviewThreads": { "totalCount": 1, "nodes": [{
                        "id": "PRRT_1", "isResolved": false, "isOutdated": false,
                        "path": "src/lib.rs", "line": 3,
                        "comments": {
                            "totalCount": 20,
                            "nodes": (1..=10).map(thread_comment).collect::<Vec<_>>()
                        },
                        "latest": { "nodes": (16..=20).map(thread_comment).collect::<Vec<_>>() }
                    }] },
                    "gate": { "nodes": [{ "commit": { "statusCheckRollup": {
                        "contexts": { "nodes": [
                            { "__typename": "CheckRun", "name": "test", "isRequired": true },
                            { "__typename": "CheckRun", "name": "lint", "isRequired": false },
                            null
                        ] }
                    } } }] }
                }
            }
        },
        "errors": [{
            "type": "FORBIDDEN",
            "path": ["r0", "p0", "gate", "nodes", 0, "commit", "statusCheckRollup",
                     "contexts", "nodes", 2, "isRequired"],
            "message": "Resource not accessible by integration"
        }]
    });
    let stdout = serde_json::to_vec(&answer).unwrap();

    let batch = parse(&plan, &stdout, "").unwrap();

    let check = |name: &str, job: u32, required: Option<bool>| CheckRun {
        name: name.to_owned(),
        workflow: Some("CI".to_owned()),
        status: CheckStatus::Passed,
        url: Some(format!(
            "https://github.com/acme/widgets/actions/runs/1/job/{job}"
        )),
        required,
    };
    let thread_post = |n: i64| Post {
        id: format!("PRRC_{n}"),
        author: "carol".to_owned(),
        author_association: Some("CONTRIBUTOR".to_owned()),
        avatar_url: None,
        at: MIDNIGHT + 3_600 + n * 60,
        body: format!("reply {n}"),
        verdict: None,
        url: format!("{url}#discussion_r{n}"),
    };
    let detail = PrDetail {
        summary: PrSummary {
            id: id.clone(),
            title: "Add widgets".to_owned(),
            url: url.to_owned(),
            state: PrState::Merged,
            change: ChangeKey {
                updated_at: "2026-11-22T06:00:00Z".to_owned(),
                head_oid: "1a2b3c4".to_owned(),
                rollup: Rollup::Passing,
            },
        },
        author: "alice".to_owned(),
        head: "widgets".to_owned(),
        base: "main".to_owned(),
        body: String::new(),
        additions: 1,
        deletions: 0,
        changed_files: 1,
        review: Review::Approved,
        checks: vec![
            check("test", 2, Some(true)),
            check("lint", 3, Some(false)),
            CheckRun {
                name: "deploy/preview".to_owned(),
                workflow: None,
                status: CheckStatus::Passed,
                url: None,
                required: None,
            },
        ],
        checks_total: 3,
        queued_workflows: 0,
        posts: vec![
            Post {
                id: "IC_1".to_owned(),
                author: "carol".to_owned(),
                author_association: Some("NONE".to_owned()),
                avatar_url: None,
                at: MIDNIGHT + 30 * 60,
                body: "nice".to_owned(),
                verdict: None,
                url: format!("{url}#issuecomment-1"),
            },
            Post {
                id: "PRR_1".to_owned(),
                author: "bob".to_owned(),
                author_association: Some("OWNER".to_owned()),
                avatar_url: Some("https://avatars.githubusercontent.com/u/2?s=72".to_owned()),
                at: MIDNIGHT + 45 * 60,
                body: String::new(),
                verdict: Some(Verdict::Approved),
                url: format!("{url}#pullrequestreview-1"),
            },
        ],
        older_posts: 0,
        threads: vec![Thread {
            id: "PRRT_1".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: Some(3),
            state: ThreadState::Open,
            comments: (1..=10).chain(16..=20).map(thread_post).collect(),
            more_comments: 5,
        }],
        older_threads: 0,
        mergeable: Mergeable::Unknown,
        merge_methods: vec![MergeMethod::Rebase, MergeMethod::Squash],
        viewer: Some("rho-bot".to_owned()),
        merged_by: Some("bob".to_owned()),
        merge_commit: Some("9f8e7d6".to_owned()),
    };
    let widgets = id.repo.clone();
    assert_eq!(
        batch,
        Batch {
            budget: Some(Budget {
                remaining: 4990,
                resets_at: MIDNIGHT + 7 * 3_600,
                cost: Some(1),
            }),
            default_branches: vec![(widgets, Some("main".to_owned()))],
            branches: Vec::new(),
            links: Vec::new(),
            details: vec![(id, Some(detail))],
            missing: Vec::new(),
        }
    );
}

// Covers: a full read counts a workflow run still going with no job queued
// or running as checks still to come, as when the docs workflow passed
// before CI made its jobs; never one whose jobs are listed running, nor an
// app's suite that GitHub keeps QUEUED for good because the app never
// reports. Suites past those read count too, as any may be such a run.
// Real answers for matthewyjiang/rho-ui, bodies and posts cut.
// Owner: pure unit (wire contract)
#[test]
fn a_workflow_without_jobs_yet_counts_as_checks_to_come() {
    use CheckStatus::{Passed, Running, Skipped};
    let fixture = |text: &str| -> Value { serde_json::from_str(text).unwrap() };
    let done = fixture(include_str!("fixtures/gh_checks_done.json"));
    let mut suites_past_page = done.clone();
    // 8 suites read of 101.
    suites_past_page["data"]["r0"]["p0"]["commits"]["nodes"][0]["commit"]["checkSuites"]
        ["totalCount"] = json!(101);
    let cases = [
        (
            "CI queued with no job yet, docs done",
            fixture(include_str!("fixtures/gh_checks_queued.json")),
            (1, vec![Passed, Skipped]),
        ),
        (
            "CI's jobs listed running",
            fixture(include_str!("fixtures/gh_checks_running.json")),
            (0, vec![Running, Running, Passed, Passed, Skipped]),
        ),
        (
            "every workflow done, six apps' suites still QUEUED",
            done,
            (0, vec![Passed, Passed, Passed, Passed, Skipped]),
        ),
        (
            "suites past those read",
            suites_past_page,
            (93, vec![Passed, Passed, Passed, Passed, Skipped]),
        ),
    ];
    for (name, answer, expected) in cases {
        let number = u32::try_from(answer["data"]["r0"]["p0"]["number"].as_u64().unwrap()).unwrap();
        let plan = FetchPlan {
            details: vec![pr("rho-ui", number)],
            ..FetchPlan::default()
        };
        let batch = parse(&plan, &serde_json::to_vec(&answer).unwrap(), "").unwrap();
        let [(_, Some(detail))] = batch.details.as_slice() else {
            panic!("{name}: the PR's detail: {:?}", batch.details);
        };
        let statuses: Vec<CheckStatus> = detail.checks.iter().map(|check| check.status).collect();
        assert_eq!((detail.queued_workflows, statuses), expected, "{name}");
        assert_eq!(detail.checks_to_come(), expected.0 > 0, "{name}");
    }
}

// Covers: an answer that can't be read whole fails the read rather than
// reading as no PR: errors with no data, an error on anything but a missing
// repository, a PR or a gate, and data that doesn't read. Output that isn't
// JSON is classified from what gh said.
// Owner: pure unit (parser)
#[test]
fn an_answer_that_does_not_read_fails_the_read() {
    let plan = FetchPlan {
        branches: vec![branch("rho-ui", "feature")],
        links: Vec::new(),
        details: Vec::new(),
    };
    let cases = [
        (
            r#"{"errors":[{"message":"Something went wrong"}]}"#,
            "",
            ReadError::Failed("Something went wrong".to_owned()),
        ),
        (
            r#"{"data":{"r0":null},"errors":[{"type":"INTERNAL","path":["r0","b0"],"message":"Timeout"}]}"#,
            "",
            ReadError::Failed("Timeout".to_owned()),
        ),
        (
            "",
            "gh: HTTP 401: Bad credentials (https://api.github.com/graphql)",
            ReadError::SignedOut(
                "HTTP 401: Bad credentials (https://api.github.com/graphql)".to_owned(),
            ),
        ),
    ];
    for (stdout, stderr, expected) in cases {
        assert_eq!(
            parse(&plan, stdout.as_bytes(), stderr),
            Err(expected),
            "{stdout} {stderr}"
        );
    }
    let unreadable = r#"{"data":{"r0":{"defaultBranchRef":null,"b0":{"nodes":[{"number":"x"}]}}}}"#;
    assert!(
        matches!(
            parse(&plan, unreadable.as_bytes(), ""),
            Err(ReadError::Failed(_))
        ),
        "a node that doesn't read"
    );
}

// Covers: a plan past the measured per-query tripwires goes out as several
// queries, each within both, dropping and reordering nothing.
// Owner: pure unit (query builder)
#[test]
fn plans_split_at_the_per_query_tripwires() {
    let plan = |branches: u32, links: u32, details: u32| FetchPlan {
        branches: (0..branches)
            .map(|n| branch("rho", &format!("b{n}")))
            .collect(),
        links: (1..=links).map(|n| pr("rho", n)).collect(),
        details: (1..=details).map(|n| pr("rho-ui", n)).collect(),
    };
    let cases = [
        ((0, 0, 0), vec![]),
        ((0, 3, 2), vec![(0, 3, 2)]),
        ((0, 150, 30), vec![(0, 100, 25), (0, 50, 5)]),
        ((60, 60, 0), vec![(60, 40, 0), (0, 20, 0)]),
        ((0, 0, 51), vec![(0, 0, 25), (0, 0, 25), (0, 0, 1)]),
    ];
    for ((branches, links, details), expected) in cases {
        let whole = plan(branches, links, details);
        let parts = split(whole.clone());
        let sizes: Vec<(usize, usize, usize)> = parts
            .iter()
            .map(|part| (part.branches.len(), part.links.len(), part.details.len()))
            .collect();
        assert_eq!(
            sizes, expected,
            "{branches} branches, {links} links, {details} details"
        );
        let rejoined = parts
            .into_iter()
            .fold(FetchPlan::default(), |mut rejoined, part| {
                rejoined.branches.extend(part.branches);
                rejoined.links.extend(part.links);
                rejoined.details.extend(part.details);
                rejoined
            });
        assert_eq!(rejoined, whole);
    }
}
