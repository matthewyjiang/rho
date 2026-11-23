use pretty_assertions::assert_eq;

use super::{evaluate, failed_read, refused_read, Report, COMMENT_WAKE_LIMIT, READ_FAILURE_LIMIT};
use crate::app::pull_requests::{
    gh::ReadError,
    model::{
        ChangeKey, CheckRun, CheckStatus, MergeMethod, Mergeable, Post, PrDetail, PrId, PrState,
        PrSummary, RepoId, Review, Rollup, Thread, ThreadState, Verdict,
    },
    state::{Baseline, Change, CheckRef, End, Failure, Gate, Owed, Remark, Reviewed, WatchState},
};

const HEAD: &str = "1a2b3c4d5e6f";
const NEW_HEAD: &str = "9f8e7d6c5b4a";
/// When the watch began, in unix milliseconds, and the same in seconds.
const START: u64 = 1_700_000_000_000;
const START_S: i64 = 1_700_000_000;
const NOW: u64 = START + 600_000;

/// An open PR on `HEAD` that merges cleanly, read as `me`, with `checks`.
fn read(checks: Vec<CheckRun>) -> PrDetail {
    PrDetail {
        summary: PrSummary {
            id: PrId {
                repo: RepoId {
                    host: "github.com".into(),
                    owner: "acme".into(),
                    name: "widgets".into(),
                },
                number: 12,
            },
            title: "Widgets".into(),
            url: "https://github.com/acme/widgets/pull/12".into(),
            state: PrState::Open,
            change: ChangeKey {
                updated_at: "2026-11-22T00:00:00Z".into(),
                head_oid: HEAD.into(),
                rollup: Rollup::Pending,
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
        checks_total: u32::try_from(checks.len()).unwrap(),
        checks,
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

fn with_head(mut detail: PrDetail, head: &str) -> PrDetail {
    detail.summary.change.head_oid = head.into();
    detail
}

fn with_mergeable(mut detail: PrDetail, mergeable: Mergeable) -> PrDetail {
    detail.mergeable = mergeable;
    detail
}

fn with_posts(mut detail: PrDetail, posts: Vec<Post>) -> PrDetail {
    detail.posts = posts;
    detail
}

/// `detail` with `unread` more checks than it lists, as past GitHub's page.
fn with_unread(mut detail: PrDetail, unread: u32) -> PrDetail {
    detail.checks_total += unread;
    detail
}

/// A check that says nothing of being required.
fn check(name: &str, status: CheckStatus) -> CheckRun {
    CheckRun {
        name: name.into(),
        workflow: None,
        status,
        url: Some(format!("https://ci.example/{name}")),
        required: None,
    }
}

fn required(name: &str, status: CheckStatus) -> CheckRun {
    CheckRun {
        required: Some(true),
        ..check(name, status)
    }
}

fn advisory(name: &str, status: CheckStatus) -> CheckRun {
    CheckRun {
        required: Some(false),
        ..check(name, status)
    }
}

fn failure(name: &str) -> CheckRef {
    CheckRef {
        name: name.into(),
        url: Some(format!("https://ci.example/{name}")),
        failure: Failure::Failed,
    }
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn post(id: &str, author: &str, at: i64) -> Post {
    Post {
        id: id.into(),
        author: author.into(),
        author_association: Some("MEMBER".into()),
        at,
        body: format!("about {id}"),
        verdict: None,
        url: format!("https://github.com/acme/widgets/pull/12#{id}"),
    }
}

/// `post(id, author, at)` as news.
fn remark(id: &str, author: &str, at: i64) -> Remark {
    Remark {
        author: author.into(),
        association: Some("MEMBER".into()),
        path: None,
        body: format!("about {id}"),
        verdict: None,
        url: format!("https://github.com/acme/widgets/pull/12#{id}"),
        at,
    }
}

/// A watch past its baseline on `HEAD`, with nothing told yet.
fn watched() -> WatchState {
    WatchState {
        baseline: Baseline::Taken,
        head_oid: Some(HEAD.into()),
        ..WatchState::new(START)
    }
}

fn quiet(next: WatchState) -> Report {
    Report {
        changes: Vec::new(),
        next,
        end: None,
    }
}

fn told(changes: Vec<Change>, next: WatchState) -> Report {
    Report {
        changes,
        next,
        end: None,
    }
}

struct Case {
    name: &'static str,
    before: WatchState,
    read: PrDetail,
    expected: Report,
}

fn assert_cases(cases: Vec<Case>) {
    for case in cases {
        assert_eq!(
            evaluate(&case.before, &case.read, NOW),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn a_baseline_is_never_news() {
    use CheckStatus::{Failed, Passed};
    let pending = WatchState::new(START);
    assert_cases(vec![
        Case {
            name: "the gate already passed",
            before: pending.clone(),
            read: read(vec![check("a", Passed), check("b", Passed)]),
            expected: quiet(WatchState {
                gate_passed: true,
                gate: names(&["a", "b"]),
                ..watched()
            }),
        },
        Case {
            name: "a failure and a conflict already there",
            before: pending.clone(),
            read: with_mergeable(
                read(vec![check("a", Failed), check("b", Passed)]),
                Mergeable::Conflicting,
            ),
            expected: quiet(WatchState {
                failed: names(&["a"]),
                conflicting: true,
                ..watched()
            }),
        },
        Case {
            name: "a comment from before the watch",
            before: pending.clone(),
            read: with_posts(read(vec![]), vec![post("c1", "alice", START_S - 60)]),
            expected: quiet(watched()),
        },
        Case {
            name: "a comment by someone else since the watch began",
            before: pending,
            read: with_posts(read(vec![]), vec![post("c1", "alice", START_S + 60)]),
            expected: told(
                vec![Change::Remarks {
                    remarks: vec![remark("c1", "alice", START_S + 60)],
                }],
                WatchState {
                    remarks_through: START_S + 60,
                    remark_ids: names(&["c1"]),
                    comment_wakes: 1,
                    ..watched()
                },
            ),
        },
    ]);
}

#[test]
fn failures_are_told_once_at_once() {
    use CheckStatus::{Failed, Passed, Pending, Running};
    assert_cases(vec![
        Case {
            name: "newly failed after the baseline",
            before: watched(),
            read: read(vec![check("a", Failed), check("b", Passed)]),
            expected: told(
                vec![Change::ChecksFailed {
                    failed: vec![failure("a")],
                }],
                WatchState {
                    failed: names(&["a"]),
                    ..watched()
                },
            ),
        },
        Case {
            name: "the same failure read again",
            before: WatchState {
                failed: names(&["a"]),
                ..watched()
            },
            read: read(vec![check("a", Failed), check("b", Passed)]),
            expected: quiet(WatchState {
                failed: names(&["a"]),
                ..watched()
            }),
        },
        Case {
            name: "a second check fails while a third still runs",
            before: WatchState {
                failed: names(&["a"]),
                ..watched()
            },
            read: read(vec![
                check("a", Failed),
                check("b", Failed),
                check("c", Running),
            ]),
            expected: told(
                vec![Change::ChecksFailed {
                    failed: vec![failure("b")],
                }],
                WatchState {
                    failed: names(&["a", "b"]),
                    owed: Owed::Everything,
                    ..watched()
                },
            ),
        },
        Case {
            // Forgotten, so its next failure is told again, as the first row.
            name: "a failed check rerun to pending",
            before: WatchState {
                failed: names(&["a"]),
                ..watched()
            },
            read: read(vec![check("a", Pending), check("b", Passed)]),
            expected: quiet(watched()),
        },
    ]);
}

#[test]
fn the_gate_passes_once() {
    use CheckStatus::{Passed, Running};
    assert_cases(vec![
        Case {
            name: "required checks pass while an advisory one runs",
            before: watched(),
            read: read(vec![required("r", Passed), advisory("adv", Running)]),
            expected: told(
                vec![Change::ChecksPassed {
                    count: 1,
                    gate: Gate::Required,
                }],
                WatchState {
                    gate_passed: true,
                    gate: names(&["r"]),
                    owed: Owed::Advisory(names(&["adv"])),
                    ..watched()
                },
            ),
        },
        Case {
            name: "none required and all pass",
            before: watched(),
            read: read(vec![advisory("a", Passed), advisory("b", Passed)]),
            expected: told(
                vec![Change::ChecksPassed {
                    count: 2,
                    gate: Gate::All,
                }],
                WatchState {
                    gate_passed: true,
                    gate: names(&["a", "b"]),
                    ..watched()
                },
            ),
        },
        Case {
            name: "a read that says nothing of required counts as none required",
            before: watched(),
            read: read(vec![check("a", Passed), check("b", Passed)]),
            expected: told(
                vec![Change::ChecksPassed {
                    count: 2,
                    gate: Gate::All,
                }],
                WatchState {
                    gate_passed: true,
                    gate: names(&["a", "b"]),
                    ..watched()
                },
            ),
        },
        Case {
            name: "every listed check passed, but more were not read",
            before: watched(),
            read: with_unread(read(vec![check("a", Passed), check("b", Passed)]), 1),
            expected: quiet(watched()),
        },
        Case {
            name: "a new passed check where none is required",
            before: WatchState {
                gate_passed: true,
                gate: names(&["a", "b"]),
                ..watched()
            },
            read: read(vec![
                check("a", Passed),
                check("b", Passed),
                check("c", Passed),
            ]),
            expected: quiet(WatchState {
                gate_passed: true,
                gate: names(&["a", "b", "c"]),
                ..watched()
            }),
        },
        Case {
            name: "a required check that shows up later, already passed",
            before: WatchState {
                gate_passed: true,
                gate: names(&["r1"]),
                ..watched()
            },
            read: read(vec![required("r1", Passed), required("r2", Passed)]),
            expected: told(
                vec![Change::ChecksPassed {
                    count: 2,
                    gate: Gate::Required,
                }],
                WatchState {
                    gate_passed: true,
                    gate: names(&["r1", "r2"]),
                    ..watched()
                },
            ),
        },
    ]);
}

#[test]
fn owed_checks_are_told_once_when_they_settle() {
    use CheckStatus::{Failed, Passed, Running};
    let gate_passed = WatchState {
        gate_passed: true,
        gate: names(&["r"]),
        ..watched()
    };
    let owing = WatchState {
        owed: Owed::Advisory(names(&["adv"])),
        ..gate_passed.clone()
    };
    assert_cases(vec![
        Case {
            name: "the owed checks all pass",
            before: owing.clone(),
            read: read(vec![required("r", Passed), advisory("adv", Passed)]),
            expected: told(
                vec![Change::ChecksFinished {
                    passed: 2,
                    failing: vec![],
                }],
                gate_passed.clone(),
            ),
        },
        Case {
            name: "the last owed check fails: one wake naming it",
            before: owing,
            read: read(vec![required("r", Passed), advisory("adv", Failed)]),
            expected: told(
                vec![Change::ChecksFinished {
                    passed: 1,
                    failing: vec![failure("adv")],
                }],
                WatchState {
                    failed: names(&["adv"]),
                    ..gate_passed.clone()
                },
            ),
        },
        Case {
            name: "later advisory churn",
            before: gate_passed.clone(),
            read: read(vec![
                required("r", Passed),
                advisory("adv", Passed),
                advisory("late", Running),
            ]),
            expected: quiet(gate_passed),
        },
        Case {
            name: "a failed gate's other checks all finish",
            before: WatchState {
                failed: names(&["a", "b"]),
                owed: Owed::Everything,
                ..watched()
            },
            read: read(vec![
                check("a", Failed),
                check("b", Failed),
                check("c", Passed),
            ]),
            expected: told(
                vec![Change::ChecksFinished {
                    passed: 1,
                    failing: vec![failure("a"), failure("b")],
                }],
                WatchState {
                    failed: names(&["a", "b"]),
                    ..watched()
                },
            ),
        },
        Case {
            // The unread checks may still run: a new failure is told, and
            // the owed wake waits.
            name: "a failed gate's listed checks finish, but more were not read",
            before: WatchState {
                failed: names(&["a"]),
                owed: Owed::Everything,
                ..watched()
            },
            read: with_unread(read(vec![check("a", Failed), check("b", Failed)]), 1),
            expected: told(
                vec![Change::ChecksFailed {
                    failed: vec![failure("b")],
                }],
                WatchState {
                    failed: names(&["a", "b"]),
                    owed: Owed::Everything,
                    ..watched()
                },
            ),
        },
    ]);
}

#[test]
fn a_new_head_starts_over() {
    use CheckStatus::{Failed, Passed, Running};
    let old_head = WatchState {
        failed: names(&["a"]),
        owed: Owed::Everything,
        conflicting: true,
        comment_wakes: 3,
        ..watched()
    };
    let new_head = WatchState {
        head_oid: Some(NEW_HEAD.into()),
        ..watched()
    };
    assert_cases(vec![
        Case {
            name: "nothing owed from the old head is told",
            before: old_head.clone(),
            read: with_mergeable(
                with_head(
                    read(vec![check("a", Running), check("b", Failed)]),
                    NEW_HEAD,
                ),
                Mergeable::Unknown,
            ),
            expected: told(
                vec![Change::ChecksFailed {
                    failed: vec![failure("b")],
                }],
                WatchState {
                    failed: names(&["b"]),
                    owed: Owed::Everything,
                    ..new_head.clone()
                },
            ),
        },
        Case {
            name: "the new head's pass is told",
            before: old_head,
            read: with_head(read(vec![check("a", Passed), check("b", Passed)]), NEW_HEAD),
            expected: told(
                vec![Change::ChecksPassed {
                    count: 2,
                    gate: Gate::All,
                }],
                WatchState {
                    gate_passed: true,
                    gate: names(&["a", "b"]),
                    ..new_head
                },
            ),
        },
    ]);
}

#[test]
fn remarks_by_others_are_told_once() {
    let review = Post {
        verdict: Some(Verdict::Approved),
        ..post("r1", "Bob", START_S + 20)
    };
    let mut on_a_line = read(vec![]);
    on_a_line.threads = vec![Thread {
        id: "t1".into(),
        path: "src/lib.rs".into(),
        line: Some(4),
        state: ThreadState::Open,
        comments: vec![
            post("l1", "carol", START_S + 30),
            post("l2", "ME", START_S + 40),
        ],
        more_comments: 0,
    }];
    assert_cases(vec![
        Case {
            name: "a comment and a review by others",
            before: watched(),
            read: with_posts(
                read(vec![]),
                vec![post("c1", "alice", START_S + 10), review],
            ),
            expected: told(
                vec![Change::Remarks {
                    remarks: vec![
                        remark("c1", "alice", START_S + 10),
                        Remark {
                            verdict: Some(Reviewed::Approved),
                            ..remark("r1", "Bob", START_S + 20)
                        },
                    ],
                }],
                WatchState {
                    remarks_through: START_S + 20,
                    remark_ids: names(&["r1"]),
                    comment_wakes: 1,
                    ..watched()
                },
            ),
        },
        Case {
            name: "a comment on a line, and the viewer's own in any case",
            before: watched(),
            read: on_a_line,
            expected: told(
                vec![Change::Remarks {
                    remarks: vec![Remark {
                        path: Some("src/lib.rs".into()),
                        ..remark("l1", "carol", START_S + 30)
                    }],
                }],
                WatchState {
                    remarks_through: START_S + 30,
                    remark_ids: names(&["l1"]),
                    comment_wakes: 1,
                    ..watched()
                },
            ),
        },
        Case {
            name: "the viewer's own",
            before: watched(),
            read: with_posts(read(vec![]), vec![post("c1", "Me", START_S + 10)]),
            expected: quiet(watched()),
        },
        Case {
            name: "an unseen post at the watermark's second",
            before: WatchState {
                remarks_through: START_S + 20,
                remark_ids: names(&["r1"]),
                ..watched()
            },
            read: with_posts(
                read(vec![]),
                vec![
                    post("r1", "bob", START_S + 20),
                    post("c2", "dave", START_S + 20),
                ],
            ),
            expected: told(
                vec![Change::Remarks {
                    remarks: vec![remark("c2", "dave", START_S + 20)],
                }],
                WatchState {
                    remarks_through: START_S + 20,
                    remark_ids: names(&["r1", "c2"]),
                    comment_wakes: 1,
                    ..watched()
                },
            ),
        },
    ]);
}

#[test]
fn a_conflict_is_told_per_onset() {
    let conflicting = WatchState {
        conflicting: true,
        ..watched()
    };
    assert_cases(vec![
        Case {
            name: "clean, then conflicting",
            before: watched(),
            read: with_mergeable(read(vec![]), Mergeable::Conflicting),
            expected: told(vec![Change::Conflicting], conflicting.clone()),
        },
        Case {
            name: "still conflicting",
            before: conflicting.clone(),
            read: with_mergeable(read(vec![]), Mergeable::Conflicting),
            expected: quiet(conflicting.clone()),
        },
        Case {
            // So the next conflict is an onset, as the first row.
            name: "clean again",
            before: conflicting.clone(),
            read: with_mergeable(read(vec![]), Mergeable::Yes),
            expected: quiet(watched()),
        },
        Case {
            name: "not worked out yet keeps what was known",
            before: conflicting.clone(),
            read: with_mergeable(read(vec![]), Mergeable::Unknown),
            expected: quiet(conflicting),
        },
    ]);
}

#[test]
fn a_merge_or_close_ends_the_watch() {
    let mut merged = read(vec![]);
    merged.summary.state = PrState::Merged;
    merged.merged_by = Some("bob".into());
    merged.merge_commit = Some("9f8e7d6aaaa".into());
    let mut closed = read(vec![]);
    closed.summary.state = PrState::Closed;
    let merged_by_bob = End::Merged {
        by: Some("bob".into()),
        commit: Some("9f8e7d6aaaa".into()),
        base: "main".into(),
    };
    assert_cases(vec![
        Case {
            name: "merged",
            before: watched(),
            read: merged.clone(),
            expected: Report {
                changes: vec![],
                next: watched(),
                end: Some(merged_by_bob.clone()),
            },
        },
        Case {
            name: "closed",
            before: watched(),
            read: closed,
            expected: Report {
                changes: vec![],
                next: watched(),
                end: Some(End::Closed),
            },
        },
        Case {
            name: "merged, read with a new comment",
            before: watched(),
            read: with_posts(merged, vec![post("c1", "alice", START_S + 10)]),
            expected: Report {
                changes: vec![Change::Remarks {
                    remarks: vec![remark("c1", "alice", START_S + 10)],
                }],
                next: WatchState {
                    remarks_through: START_S + 10,
                    remark_ids: names(&["c1"]),
                    comment_wakes: 1,
                    ..watched()
                },
                end: Some(merged_by_bob),
            },
        },
    ]);
}

#[test]
fn comment_only_wakes_are_bounded() {
    let almost = WatchState {
        comment_wakes: COMMENT_WAKE_LIMIT - 1,
        ..watched()
    };
    let commented = with_posts(read(vec![]), vec![post("c1", "bot", START_S + 10)]);
    let seen = WatchState {
        remarks_through: START_S + 10,
        remark_ids: names(&["c1"]),
        ..almost.clone()
    };
    let remarks = Change::Remarks {
        remarks: vec![remark("c1", "bot", START_S + 10)],
    };
    assert_cases(vec![
        Case {
            name: "the last comment-only report allowed",
            before: almost.clone(),
            read: commented.clone(),
            expected: Report {
                changes: vec![remarks.clone()],
                next: WatchState {
                    comment_wakes: COMMENT_WAKE_LIMIT,
                    ..seen.clone()
                },
                end: Some(End::CommentLoop),
            },
        },
        Case {
            name: "a conflict in the same report starts the count over",
            before: almost.clone(),
            read: with_mergeable(commented, Mergeable::Conflicting),
            expected: told(
                vec![remarks, Change::Conflicting],
                WatchState {
                    comment_wakes: 0,
                    conflicting: true,
                    ..seen
                },
            ),
        },
        Case {
            name: "a check report starts the count over",
            before: almost,
            read: read(vec![check("a", CheckStatus::Failed)]),
            expected: told(
                vec![Change::ChecksFailed {
                    failed: vec![failure("a")],
                }],
                WatchState {
                    failed: names(&["a"]),
                    ..watched()
                },
            ),
        },
    ]);
}

#[test]
fn refused_reads_end_the_watch_at_the_limit() {
    enum Read {
        Failed(ReadError),
        Refused(&'static str),
    }
    let failing = |read_failures| WatchState {
        read_failures,
        ..watched()
    };
    let last = READ_FAILURE_LIMIT - 1;
    let cases = [
        (
            "signed out, the last time allowed",
            last,
            Read::Failed(ReadError::SignedOut("HTTP 401: Bad credentials".into())),
            Report {
                changes: vec![],
                next: failing(READ_FAILURE_LIMIT),
                end: Some(End::Unreadable("HTTP 401: Bad credentials".into())),
            },
        ),
        (
            "gh missing, under the limit",
            last - 1,
            Read::Failed(ReadError::NotInstalled("gh is not on the PATH".into())),
            quiet(failing(last)),
        ),
        (
            "a read that answered without the PR, the last time allowed",
            last,
            Read::Refused("Could not resolve to a PullRequest"),
            Report {
                changes: vec![],
                next: failing(READ_FAILURE_LIMIT),
                end: Some(End::Unreadable("Could not resolve to a PullRequest".into())),
            },
        ),
        (
            "a transport failure starts the count over",
            last,
            Read::Failed(ReadError::Failed("gh didn't answer within 60 s".into())),
            quiet(failing(0)),
        ),
    ];
    for (name, before, outcome, expected) in cases {
        let before = failing(before);
        let report = match outcome {
            Read::Failed(error) => failed_read(&before, &error),
            Read::Refused(why) => refused_read(&before, why),
        };
        assert_eq!(report, expected, "{name}");
    }
    assert_eq!(
        evaluate(&failing(last), &read(vec![]), NOW),
        quiet(watched()),
        "a good read starts the count over"
    );
}
