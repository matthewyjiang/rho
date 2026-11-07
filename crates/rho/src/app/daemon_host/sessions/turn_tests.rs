use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

use super::{DaemonView, PromptStart, TurnTask, UNEXPECTED_STOP};
use crate::app::daemon_host::sessions::test_events::{
    approval, approval_withdrawn, plan, plan_withdrawn, question, question_withdrawn,
};
use crate::{
    app::{
        daemon_host::{
            jobs::{JobId, JobRecord, JobRegistry, JobState, JobStore, TurnEnd},
            sessions::LoadedSession,
        },
        session_runtime::{
            unattended_runtime_on, HangUntilCancel, InsertedMessage, QuestionSupport, SessionEvent,
        },
    },
    config::Config,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

fn inserted() -> SessionEvent {
    SessionEvent::MessageInserted(InsertedMessage::Unreplayed)
}

/// A job in `jobs` whose turn has started.
fn started_job(jobs: &JobRegistry) -> JobId {
    let record = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let job = record.id.clone();
    jobs.register(record).unwrap();
    jobs.start_turn(&job).unwrap();
    job
}

/// Session `job`, loaded in `root` with a provider that never answers, so
/// nothing but the test drives its turns.
async fn loaded_session(root: &Path, jobs: &Arc<JobRegistry>, job: JobId) -> Arc<LoadedSession> {
    let config = Config::default();
    let provider: Arc<dyn rho_sdk::provider::ModelProvider> = Arc::new(HangUntilCancel);
    let sdk = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let runtime =
        unattended_runtime_on(root, &config, QuestionSupport::Unavailable, sdk, provider).await;
    Arc::new(LoadedSession::new(job, Arc::clone(jobs), runtime, config))
}

/// Shows `events` to `view` as one model turn.
async fn show(view: &mut DaemonView, events: Vec<SessionEvent>) {
    let (event_sender, event_receiver) = mpsc::unbounded_channel();
    for event in events {
        event_sender.send(event).unwrap();
    }
    drop(event_sender);
    let (control, _control_receiver) = mpsc::unbounded_channel();
    view.show(event_receiver, control).await;
}

/// A turn that shows `events`, and the job's `state` and the prompt
/// request's `start` after them.
struct WaitingCase {
    name: &'static str,
    events: Vec<SessionEvent>,
    state: JobState,
    start: Option<PromptStart>,
}

/// A turn that shows `events`, then lets go of its view as `release` says;
/// the prompt request then got `start`.
struct EndCase {
    name: &'static str,
    events: Vec<SessionEvent>,
    release: Release,
    start: Option<PromptStart>,
}

// Covers: a session's job is waiting_for_input exactly while some approval,
// plan approval, or question of its turn is open, whichever kind and in
// whatever order they open and close, and running otherwise; and the prompt
// request is answered once the turn saved its message, not before.
// Owner: daemon session turn view.
#[tokio::test]
async fn the_job_waits_for_input_while_any_request_is_open() {
    let saved = || Some(PromptStart::Saved);
    let cases = vec![
        WaitingCase {
            name: "no request",
            events: vec![inserted(), SessionEvent::RunStarted],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "an open approval",
            events: vec![inserted(), approval(0)],
            state: JobState::WaitingForInput,
            start: saved(),
        },
        WaitingCase {
            name: "an approval withdrawn",
            events: vec![inserted(), approval(0), approval_withdrawn(0)],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "one of two approvals withdrawn",
            events: vec![inserted(), approval(0), approval(1), approval_withdrawn(1)],
            state: JobState::WaitingForInput,
            start: saved(),
        },
        WaitingCase {
            name: "a plan open after the question closed",
            events: vec![inserted(), question("q"), plan(0), question_withdrawn("q")],
            state: JobState::WaitingForInput,
            start: saved(),
        },
        WaitingCase {
            name: "every kind opened and withdrawn",
            events: vec![
                inserted(),
                approval(0),
                plan(0),
                question("q"),
                plan_withdrawn(0),
                approval_withdrawn(0),
                question_withdrawn("q"),
            ],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "a withdrawal of a request never opened",
            events: vec![inserted(), approval_withdrawn(3), question_withdrawn("x")],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "events before the message is saved",
            events: vec![SessionEvent::Notice {
                text: "reopened".into(),
            }],
            state: JobState::Running,
            start: None,
        },
    ];
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    for case in cases {
        let job = started_job(&jobs);
        let session = loaded_session(root.path(), &jobs, job.clone()).await;
        let (started, mut start) = oneshot::channel();
        let mut view = DaemonView::new(session, started);

        show(&mut view, case.events).await;

        assert_eq!(
            (
                jobs.get(&job).map(|record| record.state),
                start.try_recv().ok()
            ),
            (Some(case.state), case.start),
            "{}",
            case.name
        );
    }
}

/// How a test lets go of a view.
enum Release {
    /// The turn task records the turn's end.
    End(TurnEnd),
    /// The view drops without an end, as when the turn task panics.
    Drop,
}

/// An outcome clients could be told for a turn that ended as `end`; this
/// test checks only what the job records.
fn outcome_of(end: &TurnEnd) -> rho_protocol::TurnOutcome {
    match end {
        TurnEnd::Completed => rho_protocol::TurnOutcome::StepLimit {
            text: String::new(),
        },
        TurnEnd::Cancelled => rho_protocol::TurnOutcome::Cancelled,
        TurnEnd::Interrupted => rho_protocol::TurnOutcome::Interrupted,
        TurnEnd::Failed(message) => rho_protocol::TurnOutcome::Failed {
            error: rho_protocol::TurnError {
                kind: rho_protocol::TurnErrorKind::Other,
                message: message.clone(),
            },
        },
    }
}

impl Release {
    /// The end the job records once the view is let go.
    fn recorded_end(&self) -> TurnEnd {
        match self {
            Self::End(end) => end.clone(),
            Self::Drop => TurnEnd::Failed(UNEXPECTED_STOP.into()),
        }
    }
}

// Covers: the job records how the turn ended, and the prompt request learns
// how the turn began: saved once the message is, else how the turn ended
// before it (a completed turn saved its prompt); a turn interrupted before
// saving leaves the request to the daemon's stop; a view dropped without an
// end, as by a panicking turn task, records the turn failed and tells a
// request still waiting so.
// Owner: daemon session turn view.
#[tokio::test]
async fn the_turn_end_is_recorded_once_and_reaches_a_waiting_prompt() {
    let failed = || TurnEnd::Failed("provider refused the request".into());
    let cases = vec![
        EndCase {
            name: "completed after saving",
            events: vec![inserted()],
            release: Release::End(TurnEnd::Completed),
            start: Some(PromptStart::Saved),
        },
        EndCase {
            name: "failed after saving",
            events: vec![inserted()],
            release: Release::End(failed()),
            start: Some(PromptStart::Saved),
        },
        EndCase {
            name: "completed without a saved message",
            events: vec![],
            release: Release::End(TurnEnd::Completed),
            start: Some(PromptStart::Saved),
        },
        EndCase {
            name: "cancelled before saving",
            events: vec![],
            release: Release::End(TurnEnd::Cancelled),
            start: Some(PromptStart::Cancelled),
        },
        EndCase {
            name: "interrupted before saving",
            events: vec![],
            release: Release::End(TurnEnd::Interrupted),
            start: None,
        },
        EndCase {
            name: "failed before saving",
            events: vec![],
            release: Release::End(failed()),
            start: Some(PromptStart::Failed("provider refused the request".into())),
        },
        EndCase {
            name: "dropped while waiting for input",
            events: vec![inserted(), approval(0)],
            release: Release::Drop,
            start: Some(PromptStart::Saved),
        },
        EndCase {
            name: "dropped before saving",
            events: vec![],
            release: Release::Drop,
            start: Some(PromptStart::Failed(UNEXPECTED_STOP.into())),
        },
    ];
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    for case in cases {
        let job = started_job(&jobs);
        let session = loaded_session(root.path(), &jobs, job.clone()).await;
        let (started, mut start) = oneshot::channel();
        let mut view = DaemonView::new(session, started);
        show(&mut view, case.events).await;
        let expected_end = case.release.recorded_end();

        match case.release {
            Release::End(end) => view.end((outcome_of(&end), end)),
            Release::Drop => drop(view),
        }

        assert_eq!(
            (
                jobs.get(&job).map(|record| record.state),
                start.try_recv().ok()
            ),
            (Some(JobState::Ended(expected_end)), case.start),
            "{}",
            case.name
        );
    }
}

// Covers: a cancel sent before the turn begins its prompt ends the turn
// cancelled before it saves the prompt or calls the model (whose provider
// would wait for a cancel), so the prompt request learns it was cancelled
// and the session holds no message.
// Owner: daemon session turn task.
#[tokio::test]
async fn a_cancel_before_the_prompt_begins_saves_nothing() {
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let job = started_job(&jobs);
    let session = loaded_session(root.path(), &jobs, job.clone()).await;
    let cancel = rho_sdk::CancellationToken::new();
    cancel.cancel();

    let start = TurnTask {
        session: Arc::clone(&session),
        stopping: rho_sdk::CancellationToken::new(),
        cancel,
    }
    .spawn("hello".into());
    let start = tokio::time::timeout(TEST_BUDGET, start)
        .await
        .expect("the cancelled turn answers")
        .ok();
    // Released once the turn recorded its end.
    let messages = session
        .runtime
        .lock()
        .await
        .as_ref()
        .map(|runtime| runtime.history_for_tests().len());

    assert_eq!(
        (start, jobs.get(&job).map(|record| record.state), messages),
        (
            Some(PromptStart::Cancelled),
            Some(JobState::Ended(TurnEnd::Cancelled)),
            Some(0)
        )
    );
}
