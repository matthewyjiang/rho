use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{JobId, JobKind, JobState, JobSummary, SessionId};

use super::resolve;

fn job(id: &str, kind: JobKind, parent: Option<&str>, session: Option<&str>) -> JobSummary {
    JobSummary {
        id: JobId::new(id),
        kind,
        state: JobState::Running,
        workspace: PathBuf::from(format!("/work/{id}")),
        parent: parent.map(JobId::new),
        session: session.map(SessionId::new),
        workflow_run: None,
        created_at: 0,
        updated_at: 0,
        error: None,
        queue: None,
        grant: None,
        grant_limit: None,
    }
}

// Covers: an id names a session job by its own id or its session's; a
// subagent or workflow run, which does not stream its events yet, is
// refused, naming the session it runs in through any workflow between them;
// an id no job has is taken as a session id for the daemon to reopen.
// Owner: rho attach id resolution (unit).
#[test]
fn an_id_names_a_session_or_the_session_a_run_belongs_to() {
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
        job("orphan-job", JobKind::Subagent, Some("gone"), None),
    ];
    let target = Ok(SessionId::new("parent-session"));
    let cases = [
        ("session-job", target.clone()),
        ("parent-session", target),
        (
            "node-session",
            Err("job node-job is a subagent run in session parent-session; attach to the session: subagent and workflow runs do not stream their events yet (#1668)".to_owned()),
        ),
        (
            "workflow-job",
            Err("job workflow-job is a workflow run in session parent-session; attach to the session: subagent and workflow runs do not stream their events yet (#1668)".to_owned()),
        ),
        (
            "orphan-job",
            Err("job orphan-job is a subagent run: subagent and workflow runs do not stream their events yet (#1668)".to_owned()),
        ),
        ("tui-session", Ok(SessionId::new("tui-session"))),
    ];
    for (id, expected) in cases {
        let resolved = resolve(&jobs, id).map_err(|exit| exit.to_string());
        assert_eq!(resolved, expected, "{id}");
    }
}
