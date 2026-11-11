use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{JobId, JobKind, JobState, JobSummary};

use super::live_drives;
use crate::workflow::RunId;

fn drive(id: &str, run: RunId, created_at: u64, state: JobState) -> JobSummary {
    JobSummary {
        id: JobId::new(id),
        kind: JobKind::Workflow,
        state,
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
// sit beside the run its retry is driving. A run whose newest drive ended
// has no row, whatever an older drive of it says.
// Owner: daemon workflow rail (pure selection)
#[test]
fn a_run_shows_while_its_newest_drive_runs() {
    let (resumed, other, ended, failed_retry) =
        (RunId::new(), RunId::new(), RunId::new(), RunId::new());
    let jobs = [
        drive("first", resumed, 10, JobState::Failed),
        drive("other", other, 11, JobState::Queued),
        drive("retry", resumed, 12, JobState::Running),
        drive("done", ended, 13, JobState::Completed),
        drive("ran", failed_retry, 14, JobState::Running),
        drive("failed again", failed_retry, 15, JobState::Failed),
    ];

    let shown = live_drives(&jobs.iter().collect::<Vec<_>>())
        .into_iter()
        .map(|(run, job)| (run, job.id.as_str().to_owned()))
        .collect::<Vec<_>>();

    assert_eq!(
        shown,
        [(resumed, "retry".to_owned()), (other, "other".to_owned())]
    );
}
