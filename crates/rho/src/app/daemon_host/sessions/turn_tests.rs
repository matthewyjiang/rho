use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_sdk::UserInput;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

use super::{DaemonView, PromptStart, TurnLimit, TurnTask, UNEXPECTED_STOP};
use crate::app::daemon_host::sessions::test_events::{
    approval, approval_withdrawn, plan, plan_withdrawn, question, question_withdrawn,
};
use crate::{
    app::{
        daemon_host::{
            jobs::{JobId, JobRecord, JobRegistry, JobState, JobStore, TurnEnd},
            sessions::{open::OpenedSession, LoadedSession},
        },
        session_runtime::{
            unattended_runtime_on, HangUntilCancel, InsertedMessage, QuestionSupport, SessionEvent,
        },
    },
    config::Config,
    session_environment::SessionEnvironment,
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
    Arc::new(LoadedSession::new(
        &jobs.get(&job).unwrap(),
        Arc::clone(jobs),
        OpenedSession {
            runtime,
            config,
            environment: SessionEnvironment::default(),
        },
    ))
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
    /// The turn task records the turn's end: what clients are told, and
    /// what the job records.
    End((Option<rho_protocol::TurnOutcome>, TurnEnd)),
    /// The view drops without an end, as when the turn task panics.
    Drop,
}

impl Release {
    /// The end the job records once the view is let go.
    fn recorded_end(&self) -> TurnEnd {
        match self {
            Self::End((_, end)) => end.clone(),
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
    let completed = || {
        let answer = rho_protocol::TurnOutcome::Answered {
            text: "done".into(),
            stop: rho_protocol::AnswerStop::EndTurn,
        };
        (Some(answer), TurnEnd::Completed)
    };
    let failed = || {
        let (outcome, end) = super::failed(rho_protocol::TurnError {
            kind: rho_protocol::TurnErrorKind::Provider,
            message: "provider refused the request".into(),
        });
        (Some(outcome), end)
    };
    let cases = vec![
        EndCase {
            name: "completed after saving",
            events: vec![inserted()],
            release: Release::End(completed()),
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
            release: Release::End(completed()),
            start: Some(PromptStart::Saved),
        },
        EndCase {
            name: "cancelled before saving",
            events: vec![],
            release: Release::End((
                Some(rho_protocol::TurnOutcome::Cancelled),
                TurnEnd::Cancelled,
            )),
            start: Some(PromptStart::Ended(rho_protocol::TurnOutcome::Cancelled)),
        },
        EndCase {
            name: "interrupted before saving",
            events: vec![],
            release: Release::End((None, TurnEnd::Interrupted)),
            start: None,
        },
        EndCase {
            name: "failed before saving",
            events: vec![],
            release: Release::End(failed()),
            start: failed().0.map(PromptStart::Ended),
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
            start: Some(PromptStart::Ended(rho_protocol::TurnOutcome::Failed {
                error: rho_protocol::TurnError {
                    kind: rho_protocol::TurnErrorKind::Other,
                    message: UNEXPECTED_STOP.into(),
                },
            })),
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
            Release::End((outcome, end)) => view.end(outcome, end),
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
        limit: None,
    }
    .spawn(UserInput::text("hello"));
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
            Some(PromptStart::Ended(rho_protocol::TurnOutcome::Cancelled)),
            Some(JobState::Ended(TurnEnd::Cancelled)),
            Some(0)
        )
    );
}

/// A turn with a time limit of `timeout_ms`, whose session is busy until
/// the limit passed when `busy`, with a client's cancel sent before that when
/// `cancelled_first`. A turn whose session is free reaches its limit after
/// it saved its prompt, by the test moving the clock there.
struct LimitCase {
    name: &'static str,
    timeout_ms: u64,
    busy: bool,
    cancelled_first: bool,
    start: PromptStart,
    end: TurnEnd,
    saved: usize,
}

// Covers: the daemon cancels a turn still running at its time limit, and
// the turn ends timed out with the limit: one whose limit passed while it
// waited for the session, which the limit counts, saves nothing and answers
// the prompt with how it ended; one that saved first runs, its model call
// hanging, until the limit cancels it. A client's cancel that came before
// the limit stays a cancel, though the turn had not ended by then.
// Owner: daemon session turn task.
#[tokio::test]
async fn the_daemon_times_a_turn_out_at_its_limit() {
    let ended = |end: &TurnEnd| {
        PromptStart::Ended(match *end {
            TurnEnd::TimedOut { timeout_ms } => rho_protocol::TurnOutcome::TimedOut { timeout_ms },
            _ => rho_protocol::TurnOutcome::Cancelled,
        })
    };
    let cases = [
        LimitCase {
            name: "limit passed waiting for the session",
            timeout_ms: 1,
            busy: true,
            cancelled_first: false,
            start: ended(&TurnEnd::TimedOut { timeout_ms: 1 }),
            end: TurnEnd::TimedOut { timeout_ms: 1 },
            saved: 0,
        },
        LimitCase {
            name: "limit passed mid-turn",
            // Never reached in real time: the test moves the clock there.
            timeout_ms: 3_600_000,
            busy: false,
            cancelled_first: false,
            start: PromptStart::Saved,
            end: TurnEnd::TimedOut {
                timeout_ms: 3_600_000,
            },
            saved: 1,
        },
        LimitCase {
            name: "cancelled before the limit passed",
            timeout_ms: 1,
            busy: true,
            cancelled_first: true,
            start: ended(&TurnEnd::Cancelled),
            end: TurnEnd::Cancelled,
            saved: 0,
        },
    ];
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    for case in cases {
        let job = started_job(&jobs);
        let session = loaded_session(root.path(), &jobs, job.clone()).await;
        let runtime = case.busy.then(|| session.runtime.try_lock().unwrap());

        let start = TurnTask {
            session: Arc::clone(&session),
            stopping: rho_sdk::CancellationToken::new(),
            // The session's own, as a prompt passes it, which the limit's
            // cancel trips.
            cancel: session.turn().cancel.clone(),
            limit: Some(TurnLimit::starting_now(case.timeout_ms)),
        }
        .spawn(UserInput::text("hello"));
        if case.cancelled_first {
            session.cancel_turn();
        }
        if let Some(runtime) = runtime {
            // Waits on the clock, not on the turn, which waits for this.
            tokio::time::sleep(Duration::from_millis(case.timeout_ms + 1)).await;
            drop(runtime);
        }
        let start = tokio::time::timeout(TEST_BUDGET, start)
            .await
            .expect("the turn answers its prompt")
            .ok();
        if !case.busy {
            // The prompt is saved and the model call hangs: move the clock
            // to the limit rather than wait for it, then let it run again for
            // the budget below.
            tokio::time::pause();
            tokio::time::advance(Duration::from_millis(case.timeout_ms)).await;
            tokio::time::resume();
        }
        // Released once the turn recorded its end.
        let messages = tokio::time::timeout(TEST_BUDGET, session.runtime.lock())
            .await
            .expect("the limit ends the turn")
            .as_ref()
            .map(|runtime| runtime.history_for_tests().len());

        assert_eq!(
            (start, jobs.get(&job).map(|record| record.state), messages),
            (
                Some(case.start),
                Some(JobState::Ended(case.end)),
                Some(case.saved)
            ),
            "{}",
            case.name
        );
    }
}
