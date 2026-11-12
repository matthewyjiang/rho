use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{
    JobId, JobKind, JobLive, JobState, JobSummary, JobUsage, ModelRef, SessionId, SessionWorktree,
};

use super::{groups, Status};

fn job(id: &str, kind: JobKind, state: JobState, workspace: &str) -> JobSummary {
    JobSummary {
        id: JobId::new(id),
        kind,
        state,
        workspace: PathBuf::from(workspace),
        parent: None,
        session: Some(SessionId::new(format!("{id}-session"))),
        run: None,
        workflow_run: None,
        result_pending: false,
        created_at: 0,
        updated_at: 0,
        error: None,
        queue: None,
        live: None,
        grant: None,
        grant_limit: None,
        worktree: None,
        worktree_stale: false,
    }
}

fn live(title: Option<&str>) -> JobLive {
    JobLive {
        title: title.map(str::to_owned),
        model: ModelRef {
            provider: "xai".into(),
            model: "grok-fixture".into(),
        },
        turn_elapsed_ms: None,
        usage: None,
        inputs: Vec::new(),
    }
}

fn child(id: &str, parent: &str, state: JobState, workspace: &str) -> JobSummary {
    JobSummary {
        parent: Some(JobId::new(parent)),
        ..job(id, JobKind::Subagent, state, workspace)
    }
}

/// A row's job, depth, and status.
type Outlined = (String, usize, Status);

/// Each group's workspace with its rows.
fn outline(jobs: &[JobSummary], filter: &str) -> Vec<(String, Vec<Outlined>)> {
    groups(jobs, filter)
        .into_iter()
        .map(|group| {
            (
                group.workspace.to_string_lossy().into_owned(),
                group
                    .rows
                    .into_iter()
                    .map(|row| (row.job.as_str().to_owned(), row.depth, row.status))
                    .collect(),
            )
        })
        .collect()
}

// Covers: the view lists jobs under way and loaded sessions between turns,
// by project, each run under the job that started it however deep; a job
// that ended and is not followed, and old history, never show; a run whose
// parent is not listed lists as a root; a job waiting on its turn's request
// needs input.
// Owner: jobs view (pure selection).
#[test]
fn jobs_list_by_project_with_their_runs_nested() {
    let jobs = [
        JobSummary {
            live: Some(live(Some("Fix login"))),
            ..job("a", JobKind::Session, JobState::Running, "/work/a")
        },
        job("history", JobKind::Session, JobState::Completed, "/work/a"),
        JobSummary {
            live: Some(live(None)),
            ..job("b", JobKind::Session, JobState::WaitingForInput, "/work/b")
        },
        child("a-workflow", "a", JobState::Running, "/work/a"),
        child("a-node", "a-workflow", JobState::Queued, "/work/a"),
        child("a-done", "a", JobState::Completed, "/work/a"),
        child("orphan", "history", JobState::Running, "/work/a"),
        JobSummary {
            live: Some(live(None)),
            ..job("idle", JobKind::Session, JobState::Idle, "/work/a")
        },
    ];

    assert_eq!(
        outline(&jobs, ""),
        vec![
            (
                "/work/a".to_owned(),
                vec![
                    ("a".to_owned(), 0, Status::Running),
                    ("a-workflow".to_owned(), 1, Status::Running),
                    ("a-node".to_owned(), 2, Status::Queued),
                    ("orphan".to_owned(), 0, Status::Running),
                    ("idle".to_owned(), 0, Status::Idle),
                ],
            ),
            (
                "/work/b".to_owned(),
                vec![("b".to_owned(), 0, Status::NeedsInput)],
            ),
        ]
    );
}

// Covers: a filter keeps the rows whose title, status, project, or facts
// hold it, whatever its case; a row says the model, turn time, and spend
// the daemon follows, and names a job with no title by its kind and id.
// Owner: jobs view (pure selection).
#[test]
fn a_filter_keeps_matching_rows() {
    let jobs = [
        JobSummary {
            live: Some(JobLive {
                turn_elapsed_ms: Some(65_000),
                usage: Some(JobUsage {
                    used_tokens: 10,
                    window_tokens: None,
                    cost_usd_micros: Some(120_000),
                }),
                ..live(Some("Fix Login"))
            }),
            ..job("a", JobKind::Session, JobState::Running, "/work/a")
        },
        JobSummary {
            live: Some(live(None)),
            ..job("b", JobKind::Session, JobState::Idle, "/work/b")
        },
    ];
    let shown = |filter: &str| -> Vec<(String, Vec<String>)> {
        groups(&jobs, filter)
            .into_iter()
            .flat_map(|group| group.rows)
            .map(|row| (row.title, row.facts))
            .collect()
    };
    let fix = (
        "Fix Login".to_owned(),
        vec![
            "grok-fixture".to_owned(),
            "1m 05s".to_owned(),
            "$0.120".to_owned(),
        ],
    );
    let untitled = (
        "session b-sessio".to_owned(),
        vec!["grok-fixture".to_owned()],
    );

    assert_eq!(
        (
            shown("login"),
            shown("IDLE"),
            shown("work/b"),
            shown("nope")
        ),
        (
            vec![fix],
            vec![untitled.clone()],
            vec![untitled],
            Vec::new(),
        )
    );
}

// Covers: a session in a worktree lists under the project the worktree was
// added from, with the runs it started even though they work in the
// worktree, and its row names the worktree's branch first (#1673). An
// ended session whose worktree went stale lists too, flagged, and one whose
// worktree is not stale does not.
// Owner: jobs view (pure selection).
#[test]
fn a_worktree_session_lists_under_its_project_with_its_branch() {
    let worktree = SessionWorktree {
        branch: "rho/a-1234abcd".into(),
        path: PathBuf::from("/home/.rho/worktrees/a-1234abcd"),
        origin: PathBuf::from("/work/a"),
    };
    let jobs = [
        job("here", JobKind::Session, JobState::Running, "/work/a"),
        JobSummary {
            live: Some(live(Some("Refactor"))),
            worktree: Some(worktree.clone()),
            ..job(
                "isolated",
                JobKind::Session,
                JobState::Running,
                "/home/.rho/worktrees/a-1234abcd",
            )
        },
        child(
            "isolated-run",
            "isolated",
            JobState::Running,
            "/home/.rho/worktrees/a-1234abcd",
        ),
        JobSummary {
            worktree: Some(SessionWorktree {
                branch: "rho/a-5678ef90".into(),
                path: PathBuf::from("/home/.rho/worktrees/a-5678ef90"),
                ..worktree.clone()
            }),
            worktree_stale: true,
            ..job(
                "stale",
                JobKind::Session,
                JobState::Completed,
                "/home/.rho/worktrees/a-5678ef90",
            )
        },
        JobSummary {
            worktree: Some(SessionWorktree {
                branch: "rho/a-0000aaaa".into(),
                path: PathBuf::from("/home/.rho/worktrees/a-0000aaaa"),
                ..worktree.clone()
            }),
            ..job(
                "recent",
                JobKind::Session,
                JobState::Completed,
                "/home/.rho/worktrees/a-0000aaaa",
            )
        },
    ];

    // Each row's job and facts.
    type Facts = (String, Vec<String>);
    let listed: Vec<(String, Vec<Facts>)> = groups(&jobs, "")
        .into_iter()
        .map(|group| {
            (
                group.workspace.to_string_lossy().into_owned(),
                group
                    .rows
                    .into_iter()
                    .map(|row| (row.job.as_str().to_owned(), row.facts))
                    .collect(),
            )
        })
        .collect();

    assert_eq!(
        listed,
        vec![(
            "/work/a".to_owned(),
            vec![
                ("here".to_owned(), Vec::new()),
                (
                    "isolated".to_owned(),
                    vec!["rho/a-1234abcd".to_owned(), "grok-fixture".to_owned()],
                ),
                ("isolated-run".to_owned(), Vec::new()),
                (
                    "stale".to_owned(),
                    vec!["rho/a-5678ef90".to_owned(), "stale worktree".to_owned()],
                ),
            ],
        )]
    );
}
