use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{JobId, JobKind, JobState, JobSummary};

use super::newest_drives;
use crate::workflow::RunId;

fn drive(id: &str, run: RunId, created_at: u64) -> JobSummary {
    JobSummary {
        id: JobId::new(id),
        kind: JobKind::Workflow,
        state: JobState::Completed,
        workspace: PathBuf::from("/work"),
        parent: None,
        session: None,
        run: None,
        workflow_run: Some(run.to_string()),
        created_at,
        updated_at: created_at,
        error: None,
        queue: None,
        grant: None,
        grant_limit: None,
    }
}

// Covers: a run resumed has a job for each drive, and the rail shows the
// run once, as its newest drive has it, so a failed first drive does not
// sit beside the run its retry is driving; another run keeps its own row.
// Owner: daemon workflow rail (pure selection)
#[test]
fn a_run_shows_once_as_its_newest_drive() {
    let (resumed, other) = (RunId::new(), RunId::new());
    let jobs = [
        drive("first", resumed, 10),
        drive("other", other, 11),
        drive("retry", resumed, 12),
    ];

    let shown = newest_drives(&jobs.iter().collect::<Vec<_>>())
        .into_iter()
        .map(|(run, job)| (run, job.id.as_str().to_owned()))
        .collect::<Vec<_>>();

    assert_eq!(
        shown,
        [(resumed, "retry".to_owned()), (other, "other".to_owned())]
    );
}
