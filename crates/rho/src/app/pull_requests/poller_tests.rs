use std::{sync::Arc, time::Duration};

use futures_util::FutureExt as _;
use pretty_assertions::assert_eq;
use rho_protocol as wire;
use rho_sdk::SessionId;
use tempfile::TempDir;

use super::{BUDGET_FLOOR, WATCHES_PER_SESSION};
use crate::app::pull_requests::{
    checkout::Checkout,
    due::QUIET_REREAD,
    gh::ReadError,
    inbox::{
        CheckCounts, LastRead, ListedPullRequest, NewsBatch, PullRequestError, PullRequestInbox,
        Watched,
    },
    interest::{LISTED_POLL, SHOWN_POLL},
    model::{
        BranchId, Budget, ChangeKey, CheckRun, CheckStatus, FetchPlan, MergeMethod, Mergeable,
        PrDetail, PrId, PrState, PrSummary, RepoId, Review, Rollup,
    },
    record::LinkOrigin,
    service::{Detailed, PullRequests, ShowRead, ShowTarget},
    state::{Change, CheckRef, CheckTally, End, Failure, Gate, News},
    test_forge::ScriptedForge,
};

/// When each test's watches begin, in unix milliseconds.
const NOW: u64 = 1_700_000_000_000;
const MINUTE: u64 = 60_000;
/// The first tick after the watches began.
const TICK: u64 = NOW + MINUTE;
const HEAD: &str = "1a2b3c4d5e6f";

struct Fixture {
    dir: TempDir,
    forge: Arc<ScriptedForge>,
    service: PullRequests,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let forge = ScriptedForge::new();
        let service = PullRequests::open(dir.path(), forge.clone());
        Self {
            dir,
            forge,
            service,
        }
    }

    /// `session`'s inbox, its workspace no git repository.
    fn inbox(&self, session: &SessionId) -> PullRequestInbox {
        self.service.inbox(session, self.dir.path())
    }

    /// A second service over the same records, as after a daemon restart.
    fn reopen(&self) -> PullRequests {
        PullRequests::open(self.dir.path(), ScriptedForge::new())
    }
}

fn pr(number: u32) -> PrId {
    PrId {
        repo: RepoId {
            host: "github.com".into(),
            owner: "acme".into(),
            name: "widgets".into(),
        },
        number,
    }
}

fn target(number: u32) -> String {
    format!("acme/widgets#{number}")
}

fn url(number: u32) -> String {
    format!("https://github.com/acme/widgets/pull/{number}")
}

fn check(name: &str, status: CheckStatus) -> CheckRun {
    CheckRun {
        name: name.into(),
        workflow: None,
        status,
        url: Some(format!("https://ci.example/{name}")),
        required: None,
    }
}

/// PR `number`, open on `HEAD` and merging cleanly, with one check `ci`.
fn open(number: u32, ci: CheckStatus) -> PrDetail {
    let rollup = match ci {
        CheckStatus::Failed | CheckStatus::Cancelled => Rollup::Failing,
        CheckStatus::Running | CheckStatus::Pending => Rollup::Pending,
        CheckStatus::Passed | CheckStatus::Neutral | CheckStatus::Skipped => Rollup::Passing,
    };
    PrDetail {
        summary: PrSummary {
            id: pr(number),
            title: "Widgets".into(),
            url: url(number),
            state: PrState::Open,
            change: ChangeKey {
                updated_at: "2026-11-22T00:00:00Z".into(),
                head_oid: HEAD.into(),
                rollup,
            },
        },
        author: "me".into(),
        head: "feature".into(),
        base: "main".into(),
        body: String::new(),
        additions: 1,
        deletions: 1,
        changed_files: 1,
        review: Review::None,
        checks: vec![check("ci", ci)],
        checks_total: 1,
        posts: Vec::new(),
        older_posts: 0,
        threads: Vec::new(),
        older_threads: 0,
        mergeable: Mergeable::Yes,
        merge_methods: vec![MergeMethod::Squash],
        viewer: Some("me".into()),
        merged_by: None,
        merge_commit: None,
    }
}

/// `detail` after an edit that moves its summary's change key.
fn edited(mut detail: PrDetail) -> PrDetail {
    detail.summary.change.updated_at = "2026-11-22T00:05:00Z".into();
    detail
}

/// `detail`, merged by bob.
fn merged(detail: PrDetail) -> PrDetail {
    let mut detail = edited(detail);
    detail.summary.state = PrState::Merged;
    detail.mergeable = Mergeable::Unknown;
    detail.merged_by = Some("bob".into());
    detail.merge_commit = Some("9f8e7d6".into());
    detail
}

/// What a tick at [`TICK`] tells of PR `number`'s check `ci` failing, the
/// `seq`th news queued for its session.
fn ci_failed(number: u32, seq: u64) -> News {
    News {
        seq,
        pr: pr(number),
        url: url(number),
        head: Some(HEAD.into()),
        changes: vec![Change::ChecksFailed {
            failed: vec![CheckRef {
                name: "ci".into(),
                url: Some("https://ci.example/ci".into()),
                failure: Failure::Failed,
            }],
        }],
        end: None,
        at: TICK,
        pushed: false,
        checks: Some(CheckTally {
            failed: 1,
            ..CheckTally::default()
        }),
    }
}

fn batch(news: Vec<News>) -> NewsBatch {
    NewsBatch { news, dropped: 0 }
}

fn summaries(numbers: &[u32]) -> FetchPlan {
    FetchPlan {
        links: numbers.iter().copied().map(pr).collect(),
        ..FetchPlan::default()
    }
}

fn details(numbers: &[u32]) -> FetchPlan {
    FetchPlan {
        details: numbers.iter().copied().map(pr).collect(),
        ..FetchPlan::default()
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap()
}

// Covers: a tick reads a watched PR's full detail only when the watch may
// have news, and its cheap summary otherwise
// Owner: PR poller
#[tokio::test]
async fn a_tick_reads_a_detail_only_when_the_watch_may_have_news() {
    let passing = open(12, CheckStatus::Passed);
    let running = open(12, CheckStatus::Running);
    // GitHub's rollup reads failing as soon as one check fails.
    let mut failing_while_running = open(12, CheckStatus::Failed);
    failing_while_running
        .checks
        .push(check("lint", CheckStatus::Running));
    failing_while_running.checks_total = 2;
    let cases = [
        (
            "quiet, its detail a minute old",
            passing.clone(),
            passing.clone(),
            TICK,
            vec![summaries(&[12])],
        ),
        (
            "its checks were running",
            running.clone(),
            running,
            TICK,
            vec![details(&[12])],
        ),
        (
            "a check was running though the rollup read failing",
            failing_while_running.clone(),
            failing_while_running,
            TICK,
            vec![details(&[12])],
        ),
        (
            "quiet, its detail as old as the reread period",
            passing.clone(),
            passing.clone(),
            NOW + millis(QUIET_REREAD),
            vec![details(&[12])],
        ),
        (
            "its summary moved since its detail",
            passing.clone(),
            edited(passing),
            TICK,
            vec![summaries(&[12]), details(&[12])],
        ),
    ];
    for (name, baseline, now, at, expected) in cases {
        let fixture = Fixture::new();
        fixture.forge.set(baseline);
        fixture
            .inbox(&SessionId::new())
            .watch(&target(12), NOW)
            .await
            .unwrap();
        fixture.forge.set(now);
        fixture.forge.take_asked();

        fixture.service.tick(at).await;

        assert_eq!(fixture.forge.take_asked(), expected, "{name}");
    }
}

// Covers: at the budget floor rho reads nothing from the host until the
// limit resets, and a new watch says why it can't start
// Owner: PR poller
#[tokio::test]
async fn reads_stop_at_the_budget_floor_until_it_resets() {
    let fixture = Fixture::new();
    let resets_at = i64::try_from(NOW / 1000).unwrap() + 3600;
    fixture.forge.budget(Budget {
        remaining: BUDGET_FLOOR,
        resets_at,
        cost: Some(1),
    });
    fixture.forge.set(open(12, CheckStatus::Running));
    fixture.forge.set(open(13, CheckStatus::Running));
    let inbox = fixture.inbox(&SessionId::new());
    inbox.watch(&target(12), NOW).await.unwrap();
    fixture.forge.take_asked();

    fixture.service.tick(TICK).await;
    let refused = inbox.watch(&target(13), TICK).await;

    assert_eq!(
        (fixture.forge.take_asked(), refused),
        (
            Vec::<FetchPlan>::new(),
            Err(PullRequestError::Budget {
                remaining: BUDGET_FLOOR,
                floor: BUDGET_FLOOR,
                resets_at,
            })
        )
    );

    fixture.forge.budget(Budget {
        remaining: 4999,
        resets_at: resets_at + 3600,
        cost: Some(1),
    });
    let reset = u64::try_from(resets_at).unwrap() * 1000;
    fixture.service.tick(reset).await;

    assert_eq!(fixture.forge.take_asked(), vec![details(&[12])]);
}

// Covers: sessions watching one PR share each read, and each gets the news
// Owner: PR poller
#[tokio::test]
async fn sessions_watching_one_pull_request_share_its_reads() {
    let fixture = Fixture::new();
    let (first, second) = (SessionId::new(), SessionId::new());
    fixture.forge.set(open(12, CheckStatus::Running));
    for session in [&first, &second] {
        fixture
            .inbox(session)
            .watch(&target(12), NOW)
            .await
            .unwrap();
    }
    fixture.forge.set(open(12, CheckStatus::Failed));
    fixture.forge.take_asked();

    fixture.service.tick(TICK).await;

    assert_eq!(
        (
            fixture.forge.take_asked(),
            fixture.inbox(&first).take_news(),
            fixture.inbox(&second).take_news(),
        ),
        (
            vec![details(&[12])],
            batch(vec![ci_failed(12, 0)]),
            batch(vec![ci_failed(12, 0)]),
        )
    );
}

// Covers: a watch begun on a PR another session watches does not hide from
// that watch what changed since its last read
// Owner: PR poller
#[tokio::test]
async fn a_new_watch_does_not_hide_news_from_an_older_one() {
    let fixture = Fixture::new();
    let (first, second) = (SessionId::new(), SessionId::new());
    fixture.forge.set(open(12, CheckStatus::Passed));
    fixture.inbox(&first).watch(&target(12), NOW).await.unwrap();
    fixture.forge.set(edited(open(12, CheckStatus::Failed)));
    fixture
        .inbox(&second)
        .watch(&target(12), NOW + 1)
        .await
        .unwrap();

    fixture.service.tick(TICK).await;

    assert_eq!(
        (
            fixture.inbox(&first).take_news(),
            fixture.inbox(&second).take_news(),
        ),
        (batch(vec![ci_failed(12, 0)]), NewsBatch::default())
    );
}

// Covers: one tick's news on several PRs wakes a session once, together
// Owner: PR poller
#[tokio::test]
async fn a_tick_wakes_each_session_once_with_all_its_news() {
    let fixture = Fixture::new();
    let mut wake_requests = fixture.service.take_wake_requests().unwrap();
    let session = SessionId::new();
    let inbox = fixture.inbox(&session);
    for number in [12, 13] {
        fixture.forge.set(open(number, CheckStatus::Running));
        inbox.watch(&target(number), NOW).await.unwrap();
        fixture.forge.set(open(number, CheckStatus::Failed));
    }
    let news_arrived = inbox.news_owned();

    fixture.service.tick(TICK).await;

    let mut woken = Vec::new();
    while let Ok(session) = wake_requests.try_recv() {
        woken.push(session);
    }
    assert_eq!(
        (
            woken,
            news_arrived.now_or_never().is_some(),
            inbox.take_news()
        ),
        (
            vec![session],
            true,
            batch(vec![ci_failed(12, 0), ci_failed(13, 1)])
        )
    );
}

// Covers: news on a head the agent hasn't heard of says new commits came,
// so it doesn't take earlier check results for the new head's; news on the
// head it knows doesn't.
// Owner: PR service
#[tokio::test]
async fn news_on_a_new_head_says_new_commits_came() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let inbox = fixture.inbox(&session);
    fixture.forge.set(open(12, CheckStatus::Running));
    inbox.watch(&target(12), NOW).await.unwrap();
    fixture.forge.set(open(12, CheckStatus::Failed));
    fixture.service.tick(TICK).await;
    let mut pushed = open(12, CheckStatus::Failed);
    pushed.summary.change.head_oid = "9f8e7d6c5b4a".into();
    pushed.summary.change.updated_at = "2026-11-22T00:05:00Z".into();
    fixture.forge.set(pushed);

    fixture.service.tick(TICK + MINUTE).await;

    let told: Vec<(Option<String>, bool)> = inbox
        .take_news()
        .news
        .into_iter()
        .map(|news| (news.head, news.pushed))
        .collect();
    assert_eq!(
        told,
        vec![
            (Some(HEAD.into()), false),
            (Some("9f8e7d6c5b4a".into()), true)
        ]
    );
}

// Covers: news survives a daemon restart until a turn that carried it was
// applied; a turn that did not deliver it puts it back
// Owner: PR service
#[tokio::test]
async fn news_stays_queued_on_disk_until_acknowledged() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    fixture.forge.set(open(12, CheckStatus::Running));
    fixture
        .inbox(&session)
        .watch(&target(12), NOW)
        .await
        .unwrap();
    fixture.forge.set(open(12, CheckStatus::Failed));
    fixture.service.tick(TICK).await;
    let in_flight = fixture.inbox(&session).take_news();

    let restarted = fixture.reopen();
    let inbox = restarted.inbox(&session, fixture.dir.path());
    let taken = inbox.take_news();
    inbox.restore_news(&taken);
    let taken_again = inbox.take_news();
    inbox.acknowledge(&taken_again);
    let after_acknowledge = fixture
        .reopen()
        .inbox(&session, fixture.dir.path())
        .take_news();

    let news = batch(vec![ci_failed(12, 0)]);
    assert_eq!(
        (in_flight, taken, taken_again, after_acknowledge),
        (news.clone(), news.clone(), news, NewsBatch::default())
    );
}

/// What a refusal test sets up before the session watches PR 100.
enum Given {
    Merged,
    ReadFails(ReadError),
    Watching(u32),
    /// PR 100 named on a host that is neither github.com nor the
    /// workspace's origin's.
    ForeignHost,
}

// Covers: a watch that cannot work is refused at once, naming its cause
// Owner: PR service
#[tokio::test]
async fn a_watch_that_cannot_work_is_refused_with_its_cause() {
    let watching_25: Vec<(u32, bool)> = (1..=25).map(|number| (number, true)).collect();
    let cases = [
        (
            "merged already",
            Given::Merged,
            PullRequestError::AlreadyEnded {
                state: PrState::Merged,
            },
            vec![(100, false)],
        ),
        (
            "gh missing",
            Given::ReadFails(ReadError::NotInstalled("No such file".into())),
            PullRequestError::NotInstalled,
            Vec::new(),
        ),
        (
            "gh signed out",
            Given::ReadFails(ReadError::SignedOut("HTTP 401: Bad credentials".into())),
            PullRequestError::SignedOut {
                host: "github.com".into(),
            },
            Vec::new(),
        ),
        (
            "a 26th watch",
            Given::Watching(25),
            PullRequestError::TooManyWatches {
                limit: WATCHES_PER_SESSION,
                asked: 26,
            },
            watching_25,
        ),
        (
            "a host that is neither github.com nor origin's",
            Given::ForeignHost,
            PullRequestError::InvalidTarget(
                "rho only reads pull requests on github.com or this workspace's origin host, \
                 not ghe.example.com"
                    .into(),
            ),
            Vec::new(),
        ),
    ];
    for (name, given, error, links) in cases {
        let fixture = Fixture::new();
        let session = SessionId::new();
        let inbox = fixture.inbox(&session);
        let mut asked = target(100);
        fixture.forge.set(open(100, CheckStatus::Passed));
        match given {
            Given::Merged => fixture.forge.set(merged(open(100, CheckStatus::Passed))),
            Given::ReadFails(error) => fixture.forge.fail_next(error),
            Given::Watching(count) => {
                for number in 1..=count {
                    fixture.forge.set(open(number, CheckStatus::Passed));
                    inbox.watch(&target(number), NOW).await.unwrap();
                }
            }
            Given::ForeignHost => {
                asked = "https://ghe.example.com/acme/widgets/pull/100".to_owned();
            }
        }

        let refused = inbox.watch(&asked, NOW).await;

        let linked: Vec<(u32, bool)> = inbox
            .list(NOW)
            .into_iter()
            .map(|listed| (listed.pr.number, listed.watched))
            .collect();
        assert_eq!((refused, linked), (Err(error), links), "{name}");
    }
}

// Covers: a PR that merges ends its watch with one end news, and stays
// linked
// Owner: PR poller
#[tokio::test]
async fn an_end_drops_the_watch_and_keeps_the_link() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let inbox = fixture.inbox(&session);
    fixture.forge.set(open(12, CheckStatus::Passed));
    inbox.watch(&target(12), NOW).await.unwrap();
    fixture.forge.set(merged(open(12, CheckStatus::Passed)));

    fixture.service.tick(TICK).await;

    assert_eq!(
        (
            inbox.list(TICK),
            fixture.service.watches(&session),
            inbox.take_news()
        ),
        (
            vec![ListedPullRequest {
                pr: pr(12),
                url: url(12),
                origin: LinkOrigin::Agent,
                watched: false,
                last_read: Some(LastRead {
                    state: PrState::Merged,
                    rollup: Rollup::Passing,
                    age: Duration::ZERO,
                }),
                trouble: None,
            }],
            false,
            batch(vec![News {
                seq: 0,
                pr: pr(12),
                url: url(12),
                head: Some(HEAD.into()),
                changes: Vec::new(),
                end: Some(End::Merged {
                    by: Some("bob".into()),
                    commit: Some("9f8e7d6".into()),
                    base: "main".into(),
                }),
                at: TICK,
                pushed: false,
                checks: Some(CheckTally {
                    passed: 1,
                    ..CheckTally::default()
                }),
            }]),
        )
    );
}

// Covers: watching a watched PR again reports it and keeps its watch, so
// news the watch owes is still told
// Owner: PR service
#[tokio::test]
async fn watching_again_keeps_the_watch_as_it_was() {
    let fixture = Fixture::new();
    let inbox = fixture.inbox(&SessionId::new());
    fixture.forge.set(open(12, CheckStatus::Running));
    let first = inbox.watch(&target(12), NOW).await;
    fixture.forge.set(open(12, CheckStatus::Failed));
    let again = inbox.watch(&target(12), NOW + 1).await;

    fixture.service.tick(TICK).await;

    let baseline = Watched {
        pr: pr(12),
        url: url(12),
        state: PrState::Open,
        head: HEAD.into(),
        checks: CheckCounts {
            total: 1,
            passed: 0,
            running: 1,
            failed: 0,
        },
        gate: Gate::All,
        gate_checks: 1,
        mergeable: Mergeable::Yes,
        comments: 0,
        unresolved_threads: 0,
        started: true,
    };
    assert_eq!(
        (first, again, inbox.take_news()),
        (
            Ok(baseline.clone()),
            Ok(Watched {
                started: false,
                ..baseline
            }),
            batch(vec![ci_failed(12, 0)]),
        )
    );
}

// Covers: a listed session's branch is looked up and its PR shown from the
// lookup's answer; a renewed list asks nothing before the listed cadence is
// due; once the lease runs out nothing is read, and what was found stays.
// Owner: PR service
#[tokio::test]
async fn a_listed_branch_is_looked_up_and_kept_after_its_lease() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let feature = BranchId {
        repo: pr(12).repo,
        branch: "feature".into(),
    };
    fixture.forge.set(open(12, CheckStatus::Passed));
    fixture.forge.set_branch(&target(12), "feature");
    let on_feature = || Checkout::Branch(feature.clone());

    fixture.service.list(&session, on_feature(), NOW);
    fixture.service.tick(NOW).await;
    let listed = (
        fixture.forge.take_asked(),
        fixture.service.view(&session, NOW),
    );
    fixture.service.list(&session, on_feature(), NOW + MINUTE);
    fixture.service.tick(NOW + MINUTE).await;
    let renewed = (
        fixture.forge.take_asked(),
        fixture.service.view(&session, NOW + MINUTE),
    );
    let ran_out = NOW + MINUTE + 2 * millis(LISTED_POLL);
    fixture.service.tick(ran_out).await;
    let after = (
        fixture.forge.take_asked(),
        fixture.service.view(&session, ran_out),
    );

    let found = wire::SessionPullRequests::new(
        Some(wire::BranchPullRequest::new(
            Some("feature".into()),
            wire::BranchLookup::Found {
                pull_request: Box::new((&open(12, CheckStatus::Passed).summary).into()),
            },
            None,
        )),
        Vec::new(),
    );
    let lookup = FetchPlan {
        branches: vec![feature.clone()],
        ..FetchPlan::default()
    };
    assert_eq!(
        (listed, renewed, after),
        (
            (vec![lookup], found.clone()),
            (Vec::new(), found.clone()),
            (Vec::new(), found),
        )
    );
}

// Covers: linking a PR in the session's repository looks its branch up
// again on the next tick, though the listed cadence is not due: the agent
// may have just opened that PR for its branch. Otherwise a client shows the
// PR as a link beside a branch that says it has none, for up to 5 minutes.
// Owner: PR service
#[tokio::test]
async fn a_link_looks_the_branch_up_again() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let feature = BranchId {
        repo: pr(12).repo,
        branch: "feature".into(),
    };
    let on_feature = || Checkout::Branch(feature.clone());
    fixture.forge.set(open(12, CheckStatus::Passed));
    fixture.service.list(&session, on_feature(), NOW);
    fixture.service.tick(NOW).await;
    let before = fixture.service.view(&session, NOW).branch;
    fixture.forge.set_branch(&target(12), "feature");
    fixture
        .inbox(&session)
        .link(&target(12), LinkOrigin::Agent, NOW + MINUTE)
        .await
        .unwrap();

    fixture.service.tick(NOW + MINUTE).await;

    let lookup = |found| {
        Some(wire::BranchPullRequest::new(
            Some("feature".into()),
            found,
            None,
        ))
    };
    let found = wire::BranchLookup::Found {
        pull_request: Box::new((&open(12, CheckStatus::Passed).summary).into()),
    };
    assert_eq!(
        (before, fixture.service.view(&session, NOW + MINUTE).branch),
        (lookup(wire::BranchLookup::None), lookup(found))
    );
}

// Covers: a cached show of a PR a listed tick read only as a summary reads
// its detail before it answers, though the tick asked for it moments ago.
// Otherwise a panel opened just after the sidebar's poll shows no detail
// until the next tick.
// Owner: PR service
#[tokio::test]
async fn a_cached_show_reads_a_detail_the_listed_tick_did_not() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    fixture.forge.set(open(12, CheckStatus::Passed));
    fixture
        .inbox(&session)
        .link(&target(12), LinkOrigin::User, NOW)
        .await
        .unwrap();
    fixture.service.list(&session, Checkout::NotGit, NOW);
    fixture.service.tick(NOW).await;
    let shown_at = NOW + 5_000;

    let shown = fixture
        .service
        .show(
            &session,
            Checkout::NotGit,
            ShowTarget::Linked(pr(12)),
            ShowRead::Cached,
            shown_at,
        )
        .await;

    assert_eq!(
        (shown, fixture.forge.take_asked()),
        (
            Ok(Some(Detailed {
                detail: open(12, CheckStatus::Passed),
                at: shown_at,
            })),
            vec![summaries(&[12]), details(&[12])],
        )
    );
}

// Covers: a client showing a PR its session watches, read fresh a shown
// period before the tick or moments before it, while a check fails, does
// not swallow the watch's news: the fresh read is not counted as asked, so
// the next tick still reads the detail and wakes the session.
// Owner: PR poller
#[tokio::test]
async fn a_shown_pull_request_still_wakes_the_session_that_watches_it() {
    let cases = [
        ("shown a period before the tick", TICK - millis(SHOWN_POLL)),
        ("shown moments before the tick", TICK - 1_000),
    ];
    for (name, shown_at) in cases {
        let fixture = Fixture::new();
        let session = SessionId::new();
        let inbox = fixture.inbox(&session);
        fixture.forge.set(open(12, CheckStatus::Running));
        inbox.watch(&target(12), NOW).await.unwrap();
        fixture.forge.set(open(12, CheckStatus::Failed));
        fixture.forge.take_asked();

        let shown = fixture
            .service
            .show(
                &session,
                Checkout::NotGit,
                ShowTarget::Linked(pr(12)),
                ShowRead::Fresh,
                shown_at,
            )
            .await;
        fixture.service.tick(TICK).await;

        assert_eq!(
            (shown, fixture.forge.take_asked(), inbox.take_news()),
            (
                Ok(Some(Detailed {
                    detail: open(12, CheckStatus::Failed),
                    at: shown_at,
                })),
                vec![details(&[12]), details(&[12])],
                batch(vec![ci_failed(12, 0)]),
            ),
            "{name}"
        );
    }
}
