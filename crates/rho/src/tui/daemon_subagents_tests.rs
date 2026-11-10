use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{JobId, JobKind, JobState, JobSummary, SessionId};

use super::rail_runs;

const NOW: u64 = 1_000;

fn job(id: &str, kind: JobKind, parent: Option<&str>) -> JobSummary {
    JobSummary {
        id: JobId::new(id),
        kind,
        state: JobState::Running,
        workspace: PathBuf::from("/work"),
        parent: parent.map(JobId::new),
        session: None,
        run: None,
        workflow_run: None,
        created_at: 0,
        updated_at: 0,
        error: None,
        queue: None,
    }
}

fn session(id: &str, session: &str) -> JobSummary {
    JobSummary {
        session: Some(SessionId::new(session)),
        ..job(id, JobKind::Session, None)
    }
}

fn run(id: &str, parent: &str, state: JobState, updated_at: u64) -> JobSummary {
    JobSummary {
        state,
        updated_at,
        run: Some(format!("run-{id}")),
        ..job(id, JobKind::Subagent, Some(parent))
    }
}

// Covers: the rail shows the runs the followed session started itself,
// whatever order the jobs list in, and drops a finished run once it ended
// long enough ago, so a workflow's agent nodes, another session's children
// and old history never show.
// Owner: daemon subagent rail (pure selection)
#[test]
fn the_rail_takes_the_sessions_live_and_just_ended_runs() {
    let jobs = [
        // Listed before its session: both began the same second.
        run("early", "mine", JobState::Running, 0),
        // A workflow's agent node, which the workflow panel shows.
        run("node", "workflow", JobState::Running, 0),
        job("workflow", JobKind::Workflow, Some("mine")),
        session("mine", "s1"),
        session("theirs", "s2"),
        run("live", "mine", JobState::Running, 0),
        run("queued", "mine", JobState::Queued, 0),
        run("old", "mine", JobState::Completed, NOW - 60),
        run("fresh", "mine", JobState::Failed, NOW - 3),
        run("foreign", "theirs", JobState::Running, 0),
        // A record from before jobs kept the run id.
        job("unnamed", JobKind::Subagent, Some("mine")),
    ];

    assert_eq!(
        rail_runs(&jobs, "s1", NOW),
        ["run-early", "run-live", "run-queued", "run-fresh"]
    );
}
