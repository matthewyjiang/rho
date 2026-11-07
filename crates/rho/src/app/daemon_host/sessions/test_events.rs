//! The runtime's input request events, as session tests play them.

use rho_sdk::{
    ApprovalRequest, CapabilityRequest, CapabilitySource, HostChoice, HostInputId,
    HostInputRequest, HostQuestion, PathScope, SelectionMode, ToolCallId,
};

use crate::{
    app::session_runtime::{ApprovalId, PlanApprovalId, PlanApprovalRequest, SessionEvent},
    permission::PermissionMode,
};

pub(super) fn approval(n: u64) -> SessionEvent {
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

pub(super) fn approval_withdrawn(n: u64) -> SessionEvent {
    SessionEvent::ApprovalWithdrawn {
        id: ApprovalId::nth_for_tests(n),
    }
}

pub(super) fn plan(n: u64) -> SessionEvent {
    SessionEvent::PlanApprovalRequested {
        id: PlanApprovalId::nth_for_tests(n),
        request: PlanApprovalRequest {
            call_id: ToolCallId::from_string("call").unwrap(),
            plan: "the plan".into(),
            targets: vec![PermissionMode::Auto],
        },
    }
}

pub(super) fn plan_withdrawn(n: u64) -> SessionEvent {
    SessionEvent::PlanApprovalWithdrawn {
        id: PlanApprovalId::nth_for_tests(n),
    }
}

pub(super) fn question_id(name: &str) -> HostInputId {
    HostInputId::from_string(name).unwrap()
}

pub(super) fn question(name: &str) -> SessionEvent {
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

pub(super) fn question_withdrawn(name: &str) -> SessionEvent {
    SessionEvent::QuestionWithdrawn {
        id: question_id(name),
    }
}
