use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{JobId, JobKind, JobState, JobSummary, SessionId};

use super::started_by;

fn job(id: &str, kind: JobKind, parent: Option<&str>, session: Option<&str>) -> JobSummary {
    JobSummary {
        id: JobId::new(id),
        kind,
        state: JobState::WaitingForInput,
        workspace: PathBuf::from("/work"),
        parent: parent.map(JobId::new),
        session: session.map(SessionId::new),
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

// Covers: `answer` reaches only a session the calling session started: the
// session's job's parent writes the caller, as a lead's own turns or a
// subagent run in its session do; a sibling, the grandparent, the session
// itself, a session with no parent, and a caller with no session are
// refused before anyone is asked to approve the call.
// Owner: session-control answer check.
#[test]
fn only_the_session_that_started_another_may_answer_it() {
    let jobs = [
        job("lead-job", JobKind::Session, None, Some("lead")),
        job(
            "worker-job",
            JobKind::Session,
            Some("lead-job"),
            Some("worker"),
        ),
        job(
            "sibling-job",
            JobKind::Session,
            Some("lead-job"),
            Some("sibling"),
        ),
        job(
            "grandchild-job",
            JobKind::Session,
            Some("worker-job"),
            Some("grandchild"),
        ),
        job("run-job", JobKind::Subagent, Some("lead-job"), Some("run")),
        job(
            "run-child-job",
            JobKind::Session,
            Some("run-job"),
            Some("run-child"),
        ),
        job("person-job", JobKind::Session, None, Some("person")),
    ];
    let cases = [
        ("worker", Some("lead"), true),
        ("grandchild", Some("worker"), true),
        ("run-child", Some("run"), true),
        ("worker", Some("sibling"), false),
        ("grandchild", Some("lead"), false),
        ("worker", Some("worker"), false),
        ("person", Some("lead"), false),
        ("worker", None, false),
    ];
    let decided: Vec<_> = cases
        .iter()
        .map(|&(session, by, _)| {
            let allowed = started_by(
                &jobs,
                &SessionId::new(session),
                by.map(SessionId::new).as_ref(),
            );
            (session, by, allowed)
        })
        .collect();

    assert_eq!(decided, cases.to_vec());
}
