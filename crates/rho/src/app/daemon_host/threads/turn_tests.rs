use std::{path::PathBuf, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalRequest, CapabilityRequest, CapabilitySource, HostChoice, HostInputId,
    HostInputRequest, HostQuestion, PathScope, SelectionMode, ToolCallId,
};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

use super::{DaemonView, PromptStart, UNEXPECTED_STOP};
use crate::{
    app::{
        daemon_host::jobs::{JobId, JobRecord, JobRegistry, JobState, JobStore, TurnEnd},
        session_runtime::{
            ApprovalId, InsertedMessage, PlanApprovalId, PlanApprovalRequest, SessionEvent,
        },
    },
    permission::PermissionMode,
};

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
    let record = JobRecord::new_thread(PathBuf::from("/work"), rho_sdk::SessionId::new());
    let job = record.id.clone();
    jobs.register(record).unwrap();
    jobs.start_turn(&job).unwrap();
    job
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

/// (case, the model turns' events, job state after them, prompt answer).
type WaitingCase = (
    &'static str,
    Vec<Vec<SessionEvent>>,
    JobState,
    Option<PromptStart>,
);
/// (case, events, how the turn releases, recorded end, prompt answer).
type EndCase = (
    &'static str,
    Vec<SessionEvent>,
    Release,
    TurnEnd,
    Option<PromptStart>,
);

// Covers: a thread's job is waiting_for_input exactly while some approval,
// plan approval, or question of its turn is open, whichever kind and in
// whatever order they open and close, including across the model turns a
// plan handoff splits a turn into, and running otherwise; and the prompt
// request is answered once the turn saved its message, not before.
// Owner: daemon thread turn view.
#[tokio::test]
async fn the_job_waits_for_input_while_any_request_is_open() {
    let saved = Some(PromptStart::Saved);
    let cases: Vec<WaitingCase> = vec![
        (
            "no request",
            vec![vec![inserted(), SessionEvent::RunStarted]],
            JobState::Running,
            saved.clone(),
        ),
        (
            "an open approval",
            vec![vec![inserted(), approval(0)]],
            JobState::WaitingForInput,
            saved.clone(),
        ),
        (
            "an approval withdrawn",
            vec![vec![inserted(), approval(0), approval_withdrawn(0)]],
            JobState::Running,
            saved.clone(),
        ),
        (
            "one of two approvals withdrawn",
            vec![vec![
                inserted(),
                approval(0),
                approval(1),
                approval_withdrawn(1),
            ]],
            JobState::WaitingForInput,
            saved.clone(),
        ),
        (
            "a plan open after the question closed",
            vec![vec![
                inserted(),
                question("q"),
                plan(0),
                question_withdrawn("q"),
            ]],
            JobState::WaitingForInput,
            saved.clone(),
        ),
        (
            "every kind opened and withdrawn",
            vec![vec![
                inserted(),
                approval(0),
                plan(0),
                question("q"),
                plan_withdrawn(0),
                approval_withdrawn(0),
                question_withdrawn("q"),
            ]],
            JobState::Running,
            saved.clone(),
        ),
        (
            "a withdrawal of a request never opened",
            vec![vec![
                inserted(),
                approval_withdrawn(3),
                question_withdrawn("x"),
            ]],
            JobState::Running,
            saved.clone(),
        ),
        (
            "a plan still open when the next model turn starts",
            vec![vec![inserted(), plan(0)], vec![SessionEvent::RunStarted]],
            JobState::WaitingForInput,
            saved.clone(),
        ),
        (
            "a plan withdrawn in the next model turn",
            vec![vec![inserted(), plan(0)], vec![plan_withdrawn(0)]],
            JobState::Running,
            saved,
        ),
        (
            "events before the message is saved",
            vec![vec![SessionEvent::Notice {
                text: "reopened".into(),
            }]],
            JobState::Running,
            None,
        ),
    ];
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    for (name, model_turns, expected_state, expected_start) in cases {
        let job = started_job(&jobs);
        let (started, mut start) = oneshot::channel();
        let mut view = DaemonView::new(job.clone(), Arc::clone(&jobs), started);

        for events in model_turns {
            show(&mut view, events).await;
        }

        assert_eq!(
            (
                jobs.get(&job).map(|record| record.state),
                start.try_recv().ok()
            ),
            (Some(expected_state), expected_start),
            "{name}"
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

// Covers: the job records how the turn ended, and the prompt request learns
// how the turn began: saved once the message is, else how the turn ended
// before it (a completed turn saved its prompt); a view dropped without an
// end, as by a panicking turn task, records the turn failed, and a request
// still waiting then gets no answer.
// Owner: daemon thread turn view.
#[tokio::test]
async fn the_turn_end_is_recorded_once_and_reaches_a_waiting_prompt() {
    let failed = || TurnEnd::Failed("provider refused the request".into());
    let unexpected = || TurnEnd::Failed(UNEXPECTED_STOP.into());
    let cases: Vec<EndCase> = vec![
        (
            "completed after saving",
            vec![inserted()],
            Release::End(TurnEnd::Completed),
            TurnEnd::Completed,
            Some(PromptStart::Saved),
        ),
        (
            "failed after saving",
            vec![inserted()],
            Release::End(failed()),
            failed(),
            Some(PromptStart::Saved),
        ),
        (
            "completed without a saved message",
            vec![],
            Release::End(TurnEnd::Completed),
            TurnEnd::Completed,
            Some(PromptStart::Saved),
        ),
        (
            "cancelled before saving",
            vec![],
            Release::End(TurnEnd::Cancelled),
            TurnEnd::Cancelled,
            Some(PromptStart::Cancelled),
        ),
        (
            "interrupted before saving",
            vec![],
            Release::End(TurnEnd::Interrupted),
            TurnEnd::Interrupted,
            Some(PromptStart::Interrupted),
        ),
        (
            "failed before saving",
            vec![],
            Release::End(failed()),
            failed(),
            Some(PromptStart::Failed("provider refused the request".into())),
        ),
        (
            "dropped while waiting for input",
            vec![inserted(), approval(0)],
            Release::Drop,
            unexpected(),
            Some(PromptStart::Saved),
        ),
        (
            "dropped before saving",
            vec![],
            Release::Drop,
            unexpected(),
            None,
        ),
    ];
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    for (name, events, release, expected_end, expected_start) in cases {
        let job = started_job(&jobs);
        let (started, mut start) = oneshot::channel();
        let mut view = DaemonView::new(job.clone(), Arc::clone(&jobs), started);
        show(&mut view, events).await;

        match release {
            Release::End(end) => view.end(end),
            Release::Drop => drop(view),
        }

        assert_eq!(
            (
                jobs.get(&job).map(|record| record.state),
                start.try_recv().ok()
            ),
            (Some(JobState::Ended(expected_end)), expected_start),
            "{name}"
        );
    }
}
