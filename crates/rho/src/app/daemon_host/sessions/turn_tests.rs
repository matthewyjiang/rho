use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalRequest, CapabilityRequest, CapabilitySource, HostChoice, HostInputId,
    HostInputRequest, HostQuestion, PathScope, SelectionMode, ToolCallId,
};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

use super::{DaemonView, PromptStart, TurnTask, UNEXPECTED_STOP};
use crate::{
    app::{
        daemon_host::{
            jobs::{JobId, JobRecord, JobRegistry, JobState, JobStore, TurnEnd},
            sessions::LoadedSession,
        },
        session_runtime::{
            unattended_runtime_on, ApprovalId, HangUntilCancel, InsertedMessage, PlanApprovalId,
            PlanApprovalRequest, QuestionSupport, SessionEvent,
        },
    },
    config::Config,
    permission::PermissionMode,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

fn approval(n: u64) -> SessionEvent {
    SessionEvent::ApprovalRequested {
        id: ApprovalId::nth_for_tests(n),
        request: ApprovalRequest::new(
            CapabilityRequest::write_path(
                "/workspace/file.rs",
                PathScope::PrimaryWorkspace,
                CapabilitySource::built_in_tool("write"),
            ),
            "edit file",
        ),
    }
}

fn approval_withdrawn(n: u64) -> SessionEvent {
    SessionEvent::ApprovalWithdrawn {
        id: ApprovalId::nth_for_tests(n),
    }
}

fn plan(n: u64) -> SessionEvent {
    SessionEvent::PlanApprovalRequested {
        id: PlanApprovalId::nth_for_tests(n),
        request: PlanApprovalRequest {
            call_id: ToolCallId::from_string("call").unwrap(),
            plan: "the plan".into(),
            targets: vec![PermissionMode::Auto],
        },
    }
}

fn plan_withdrawn(n: u64) -> SessionEvent {
    SessionEvent::PlanApprovalWithdrawn {
        id: PlanApprovalId::nth_for_tests(n),
    }
}

fn question_id(name: &str) -> HostInputId {
    HostInputId::from_string(name).unwrap()
}

fn question(name: &str) -> SessionEvent {
    let question = HostQuestion::new(
        "answer",
        "choose",
        vec![HostChoice::new("yes", "yes")],
        SelectionMode::One,
    )
    .unwrap();
    SessionEvent::QuestionAsked {
        id: question_id(name),
        call_id: None,
        request: HostInputRequest::questionnaire("ask", vec![question]).unwrap(),
    }
}

fn question_withdrawn(name: &str) -> SessionEvent {
    SessionEvent::QuestionWithdrawn {
        id: question_id(name),
    }
}

fn inserted() -> SessionEvent {
    SessionEvent::MessageInserted(InsertedMessage::Unreplayed)
}

/// A job in `jobs` whose turn has started.
fn started_job(jobs: &JobRegistry) -> JobId {
    let record = JobRecord::new_session(PathBuf::from("/work"), rho_sdk::SessionId::new());
    let job = record.id.clone();
    jobs.register(record).unwrap();
    jobs.start_turn(&job).unwrap();
    job
}

/// Session `job`, loaded in `root` with a provider that never answers, so
/// nothing but the test drives its turns.
async fn loaded_session(root: &Path, job: JobId) -> Arc<LoadedSession> {
    let config = Config::default();
    let provider: Arc<dyn rho_sdk::provider::ModelProvider> = Arc::new(HangUntilCancel);
    let sdk = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let runtime =
        unattended_runtime_on(root, &config, QuestionSupport::Unavailable, sdk, provider).await;
    Arc::new(LoadedSession::new(job, runtime, config))
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

/// A turn whose model turns show `model_turns`, and the job's `state` and
/// the prompt request's `start` after them.
struct WaitingCase {
    name: &'static str,
    model_turns: Vec<Vec<SessionEvent>>,
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
// whatever order they open and close, including across the model turns a
// plan handoff splits a turn into, and running otherwise; and the prompt
// request is answered once the turn saved its message, not before.
// Owner: daemon session turn view.
#[tokio::test]
async fn the_job_waits_for_input_while_any_request_is_open() {
    let saved = || Some(PromptStart::Saved);
    let cases = vec![
        WaitingCase {
            name: "no request",
            model_turns: vec![vec![inserted(), SessionEvent::RunStarted]],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "an open approval",
            model_turns: vec![vec![inserted(), approval(0)]],
            state: JobState::WaitingForInput,
            start: saved(),
        },
        WaitingCase {
            name: "an approval withdrawn",
            model_turns: vec![vec![inserted(), approval(0), approval_withdrawn(0)]],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "one of two approvals withdrawn",
            model_turns: vec![vec![
                inserted(),
                approval(0),
                approval(1),
                approval_withdrawn(1),
            ]],
            state: JobState::WaitingForInput,
            start: saved(),
        },
        WaitingCase {
            name: "a plan open after the question closed",
            model_turns: vec![vec![
                inserted(),
                question("q"),
                plan(0),
                question_withdrawn("q"),
            ]],
            state: JobState::WaitingForInput,
            start: saved(),
        },
        WaitingCase {
            name: "every kind opened and withdrawn",
            model_turns: vec![vec![
                inserted(),
                approval(0),
                plan(0),
                question("q"),
                plan_withdrawn(0),
                approval_withdrawn(0),
                question_withdrawn("q"),
            ]],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "a withdrawal of a request never opened",
            model_turns: vec![vec![
                inserted(),
                approval_withdrawn(3),
                question_withdrawn("x"),
            ]],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "a plan still open when the next model turn starts",
            model_turns: vec![vec![inserted(), plan(0)], vec![SessionEvent::RunStarted]],
            state: JobState::WaitingForInput,
            start: saved(),
        },
        WaitingCase {
            name: "a plan withdrawn in the next model turn",
            model_turns: vec![vec![inserted(), plan(0)], vec![plan_withdrawn(0)]],
            state: JobState::Running,
            start: saved(),
        },
        WaitingCase {
            name: "events before the message is saved",
            model_turns: vec![vec![SessionEvent::Notice {
                text: "reopened".into(),
            }]],
            state: JobState::Running,
            start: None,
        },
    ];
    let root = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    for case in cases {
        let job = started_job(&jobs);
        let session = loaded_session(root.path(), job.clone()).await;
        let (started, mut start) = oneshot::channel();
        let mut view = DaemonView::new(session, Arc::clone(&jobs), started);

        for events in case.model_turns {
            show(&mut view, events).await;
        }

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
        let session = loaded_session(root.path(), job.clone()).await;
        let (started, mut start) = oneshot::channel();
        let mut view = DaemonView::new(session, Arc::clone(&jobs), started);
        show(&mut view, case.events).await;
        let expected_end = case.release.recorded_end();

        match case.release {
            Release::End(end) => view.end(end),
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
    let session = loaded_session(root.path(), job.clone()).await;
    let cancel = rho_sdk::CancellationToken::new();
    cancel.cancel();

    let start = TurnTask {
        jobs: Arc::clone(&jobs),
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
