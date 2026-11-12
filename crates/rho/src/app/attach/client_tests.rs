use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{JobId, JobKind, JobState, JobSummary, SessionId};

use super::{resolve, Target};

fn job(id: &str, kind: JobKind, parent: Option<&str>, session: Option<&str>) -> JobSummary {
    JobSummary {
        id: JobId::new(id),
        kind,
        state: JobState::Running,
        workspace: PathBuf::from(format!("/work/{id}")),
        parent: parent.map(JobId::new),
        session: session.map(SessionId::new),
        run: None,
        workflow_run: None,
        result_pending: false,
        created_at: 0,
        updated_at: 0,
        error: None,
        queue: None,
        grant: None,
        grant_limit: None,
    }
}

// Covers: an id names a session by its job's id or its own; a subagent
// run, a workflow's agent node included, by its job's id or the id of the
// session it writes, the latest run's when several wrote it; a run on
// another runtime and a workflow run's drive
// are refused, naming the session they run in; an id no job has is taken
// as a session id for the daemon to reopen.
// Owner: rho attach id resolution (unit).
#[test]
fn an_id_names_a_session_or_a_run() {
    let jobs = [
        job(
            "session-job",
            JobKind::Session,
            None,
            Some("parent-session"),
        ),
        job("workflow-job", JobKind::Workflow, Some("session-job"), None),
        job(
            "node-job",
            JobKind::Subagent,
            Some("workflow-job"),
            Some("node-session"),
        ),
        // The node ran again in the same session.
        job(
            "node-job-again",
            JobKind::Subagent,
            Some("workflow-job"),
            Some("node-session"),
        ),
        job("cli-job", JobKind::Subagent, Some("session-job"), None),
        job("orphan-job", JobKind::Subagent, Some("gone"), None),
    ];
    let session = Ok(Target::Session(SessionId::new("parent-session")));
    let node = Ok(Target::Run {
        job: JobId::new("node-job"),
        session: SessionId::new("node-session"),
    });
    let cases = [
        ("session-job", session.clone()),
        ("parent-session", session),
        ("node-job", node),
        (
            "node-session",
            Ok(Target::Run {
                job: JobId::new("node-job-again"),
                session: SessionId::new("node-session"),
            }),
        ),
        (
            "workflow-job",
            Err("job workflow-job drives a workflow run, whose events are its agent nodes' jobs'; it runs in session parent-session".to_owned()),
        ),
        (
            "cli-job",
            Err("job cli-job is a subagent run on another runtime, which keeps no rho session to follow; it runs in session parent-session".to_owned()),
        ),
        (
            "orphan-job",
            Err("job orphan-job is a subagent run on another runtime, which keeps no rho session to follow".to_owned()),
        ),
        ("tui-session", Ok(Target::Session(SessionId::new("tui-session")))),
    ];
    for (id, expected) in cases {
        let resolved = resolve(&jobs, id).map_err(|exit| exit.to_string());
        assert_eq!(resolved, expected, "{id}");
    }
}
