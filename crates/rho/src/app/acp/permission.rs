use std::sync::atomic::{AtomicU64, Ordering};

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption, SessionId,
    SessionMode, SessionModeId, SetSessionConfigOptionRequest, ToolCallContent, ToolCallId,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::Error as AcpError;

use crate::{
    permission::PermissionMode,
    tools::plan_exit::{PlanPermissionAnswer, PlanPermissionRequest},
};
use rho_sdk::CapabilityKind;

/// Config option id for the permission mode. ACP clients that support config
/// options ignore the legacy `modes` field, so the mode is advertised both ways.
pub(super) const MODE_CONFIG_ID: &str = "mode";

const OPTION_ALLOW_ONCE: &str = "allow_once";
const OPTION_ALLOW_ALWAYS: &str = "allow_always";
const OPTION_REJECT_ONCE: &str = "reject_once";
const OPTION_KEEP_PLANNING: &str = "keep_planning";

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

/// The `mode` config option: the same permission modes as the legacy `modes`
/// field, with the same ids, so both stay in sync.
pub(super) fn mode_config_option(current: PermissionMode) -> SessionConfigOption {
    SessionConfigOption::select(
        MODE_CONFIG_ID,
        "Mode",
        current.as_str(),
        PermissionMode::ALL
            .into_iter()
            .map(|mode| SessionConfigSelectOption::new(mode.as_str(), mode.label()))
            .collect::<Vec<_>>(),
    )
    .description("Which tool calls run without asking.")
    .category(SessionConfigOptionCategory::Mode)
}

/// Parses a `mode` config value into a permission mode.
pub(super) fn parse_mode_config_request(
    request: &SetSessionConfigOptionRequest,
) -> Result<PermissionMode, AcpError> {
    let Some(value) = request.value.as_value_id() else {
        return Err(AcpError::invalid_params().data("mode requires a select value id"));
    };
    parse_mode_id(value.0.as_ref()).ok_or_else(|| {
        AcpError::invalid_params().data(format!("unknown session mode '{}'", value.0.as_ref()))
    })
}

pub(super) fn next_placeholder_tool_call_id(counter: &AtomicU64) -> String {
    format!("pending-{}", counter.fetch_add(1, Ordering::Relaxed) + 1)
}

pub(super) fn permission_request(
    session_id: &SessionId,
    request: &rho_sdk::ApprovalRequest,
    placeholder_tool_call_id: &str,
) -> RequestPermissionRequest {
    let tool_call_id = request
        .tool_call_id()
        .map(|id| id.as_str().to_string())
        .unwrap_or_else(|| placeholder_tool_call_id.to_string());
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

/// The plan approval prompt, drawn on the `exit_plan_mode` call's card with
/// the plan as its content. Each option approves into one mode, by mode id,
/// so hosts show what each choice grants; "keep planning" rejects.
pub(super) fn plan_approval_request(
    session_id: &SessionId,
    request: &PlanPermissionRequest,
) -> RequestPermissionRequest {
    let mut options = request
        .targets
        .iter()
        .map(|&mode| {
            // Hosts style "always" as the broader grant; supervised still
            // asks before each change, so it is the "once" choice.
            let kind = match mode {
                PermissionMode::Supervised => PermissionOptionKind::AllowOnce,
                PermissionMode::AllowEdits | PermissionMode::Auto | PermissionMode::Bypass => {
                    PermissionOptionKind::AllowAlways
                }
                PermissionMode::Plan => PermissionOptionKind::RejectOnce,
            };
            PermissionOption::new(mode.as_str(), format!("Approve → {}", mode.label()), kind)
        })
        .collect::<Vec<_>>();
    options.push(PermissionOption::new(
        OPTION_KEEP_PLANNING,
        "Keep planning",
        PermissionOptionKind::RejectOnce,
    ));
    RequestPermissionRequest::new(
        session_id.clone(),
        ToolCallUpdate::new(
            ToolCallId::new(request.call_id.as_str()),
            ToolCallUpdateFields::new()
                .title("Plan ready. How should Rho continue?")
                .kind(ToolKind::SwitchMode)
                .content(vec![ToolCallContent::from(request.plan.clone())]),
        ),
        options,
    )
}

/// The user's plan decision. Anything but an offered mode, including a
/// cancelled prompt, keeps planning.
pub(super) fn plan_answer_for(
    outcome: &RequestPermissionOutcome,
    targets: &[PermissionMode],
) -> PlanPermissionAnswer {
    let RequestPermissionOutcome::Selected(selected) = outcome else {
        return PlanPermissionAnswer::KeepPlanning;
    };
    targets
        .iter()
        .copied()
        .find(|mode| mode.as_str() == selected.option_id.0.as_ref())
        .map_or(
            PlanPermissionAnswer::KeepPlanning,
            PlanPermissionAnswer::Approve,
        )
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
