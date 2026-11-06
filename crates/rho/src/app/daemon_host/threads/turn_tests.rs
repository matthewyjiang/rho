use std::{path::PathBuf, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalRequest, CapabilityRequest, CapabilitySource, HostChoice, HostInputId,
    HostInputRequest, HostQuestion, PathScope, SelectionMode, ToolCallId,
};
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

use super::DaemonView;
use crate::{
    app::{
        daemon_host::jobs::{JobRecord, JobRegistry, JobState, JobStore},
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

/// Whether the prompt request was told its turn started.
#[derive(Debug, PartialEq, Eq)]
enum Started {
    Answered,
    NotYet,
}

// Covers: a thread's job is waiting_for_input exactly while some approval,
// plan approval, or question of its turn is open, whichever kind and in
// whatever order they open and close, and running otherwise; and the
// prompt request is answered once the turn saved its message, not before.
// Owner: daemon thread turn view.
#[tokio::test]
async fn the_job_waits_for_input_while_any_request_is_open() {
    let cases: Vec<(&str, Vec<SessionEvent>, JobState, Started)> = vec![
        (
            "no request",
            vec![inserted(), SessionEvent::RunStarted],
            JobState::Running,
            Started::Answered,
        ),
        (
            "an open approval",
            vec![inserted(), approval(0)],
            JobState::WaitingForInput,
            Started::Answered,
        ),
        (
            "an approval withdrawn",
            vec![inserted(), approval(0), approval_withdrawn(0)],
            JobState::Running,
            Started::Answered,
        ),
        (
            "one of two approvals withdrawn",
            vec![inserted(), approval(0), approval(1), approval_withdrawn(1)],
            JobState::WaitingForInput,
            Started::Answered,
        ),
        (
            "a plan open after the question closed",
            vec![inserted(), question("q"), plan(0), question_withdrawn("q")],
            JobState::WaitingForInput,
            Started::Answered,
        ),
        (
            "every kind opened and withdrawn",
            vec![
                inserted(),
                approval(0),
                plan(0),
                question("q"),
                plan_withdrawn(0),
                approval_withdrawn(0),
                question_withdrawn("q"),
            ],
            JobState::Running,
            Started::Answered,
        ),
        (
            "a withdrawal of a request never opened",
            vec![inserted(), approval_withdrawn(3), question_withdrawn("x")],
            JobState::Running,
            Started::Answered,
        ),
        (
            "events before the message is saved",
            vec![SessionEvent::Notice {
                text: "reopened".into(),
            }],
            JobState::Running,
            Started::NotYet,
        ),
    ];
    let home = TempDir::new().unwrap();
    let jobs = Arc::new(JobRegistry::open(JobStore::in_rho_home(home.path())));
    for (name, events, expected_state, expected_started) in cases {
        let record = JobRecord::new_thread(PathBuf::from("/work"), rho_sdk::SessionId::new());
        let job = record.id.clone();
        jobs.register(record).unwrap();
        jobs.start_turn(&job).unwrap();
        let (started, mut started_receiver) = oneshot::channel();
        let mut view = DaemonView {
            job: job.clone(),
            jobs: Arc::clone(&jobs),
            started: Some(started),
        };
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        for event in events {
            event_sender.send(event).unwrap();
        }
        drop(event_sender);
        let (control, _control_receiver) = mpsc::unbounded_channel();

        view.show(event_receiver, control).await;

        let started = match started_receiver.try_recv() {
            Ok(()) => Started::Answered,
            Err(_) => Started::NotYet,
        };
        assert_eq!(
            (jobs.get(&job).map(|record| record.state), started),
            (Some(expected_state), expected_started),
            "{name}"
        );
    }
}
