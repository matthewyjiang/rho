use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SessionId, SessionMode, SessionModeId, ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind,
};

use crate::permission::PermissionMode;
use rho_sdk::CapabilityKind;

const OPTION_ALLOW_ONCE: &str = "allow_once";
const OPTION_ALLOW_ALWAYS: &str = "allow_always";
const OPTION_REJECT_ONCE: &str = "reject_once";

pub(super) fn mode_list() -> Vec<SessionMode> {
    PermissionMode::ALL
        .into_iter()
        .map(|mode| SessionMode::new(mode_id(mode), mode.label()))
        .collect()
}

pub(super) fn mode_id(mode: PermissionMode) -> SessionModeId {
    SessionModeId::new(mode.as_str())
}

pub(super) fn parse_mode_id(id: &str) -> Option<PermissionMode> {
    PermissionMode::ALL
        .into_iter()
        .find(|mode| mode.as_str() == id)
}

pub(super) fn permission_request(
    session_id: &SessionId,
    request: &rho_sdk::ApprovalRequest,
) -> RequestPermissionRequest {
    let tool_call_id = request
        .tool_call_id()
        .map(|id| id.as_str().to_string())
        .unwrap_or_else(|| "unknown".into());
    RequestPermissionRequest::new(
        session_id.clone(),
        ToolCallUpdate::new(
            ToolCallId::new(tool_call_id),
            ToolCallUpdateFields::new()
                .title(request.reason())
                .kind(tool_kind_for_capability(request.capability().kind())),
        ),
        permission_options(),
    )
}

pub(super) fn decision_for(outcome: &RequestPermissionOutcome) -> rho_sdk::ApprovalDecision {
    match outcome {
        RequestPermissionOutcome::Cancelled => rho_sdk::ApprovalDecision::Deny {
            reason: "permission request cancelled".into(),
        },
        RequestPermissionOutcome::Selected(selected) => match selected.option_id.0.as_ref() {
            OPTION_ALLOW_ONCE => rho_sdk::ApprovalDecision::AllowOnce,
            OPTION_ALLOW_ALWAYS => rho_sdk::ApprovalDecision::AllowForSession,
            OPTION_REJECT_ONCE => rho_sdk::ApprovalDecision::Deny {
                reason: "permission rejected".into(),
            },
            _ => rho_sdk::ApprovalDecision::Deny {
                reason: "unknown permission option".into(),
            },
        },
        _ => rho_sdk::ApprovalDecision::Deny {
            reason: "unknown permission outcome".into(),
        },
    }
}

fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption::new(
            OPTION_ALLOW_ONCE,
            "Allow once",
            PermissionOptionKind::AllowOnce,
        ),
        PermissionOption::new(
            OPTION_ALLOW_ALWAYS,
            "Allow for this session",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(
            OPTION_REJECT_ONCE,
            "Reject once",
            PermissionOptionKind::RejectOnce,
        ),
    ]
}

fn tool_kind_for_capability(kind: CapabilityKind) -> ToolKind {
    match kind {
        CapabilityKind::Read | CapabilityKind::InstructionDiscovery => ToolKind::Read,
        CapabilityKind::Write => ToolKind::Edit,
        CapabilityKind::Process => ToolKind::Execute,
        CapabilityKind::Network => ToolKind::Fetch,
        CapabilityKind::Skill => ToolKind::Other,
        _ => ToolKind::Other,
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
