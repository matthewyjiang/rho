//! A daemon approval (`wire::Ask::Approval`) or plan approval
//! (`wire::Ask::PlanApproval`) as ACP `session/request_permission`, and the
//! host's pick as the daemon's answer.

use std::sync::atomic::{AtomicU64, Ordering};

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SessionId, SessionModeId, ToolCallContent, ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind,
};
use rho_protocol::{self as wire, ApprovalAsk, InputAnswer, Operation, PlanApprovalAsk};

use crate::{permission::PermissionMode, tools::plan_exit};

const OPTION_ALLOW_ONCE: &str = "allow_once";
const OPTION_ALLOW_ALWAYS: &str = "allow_always";
const OPTION_REJECT_ONCE: &str = "reject_once";
const OPTION_KEEP_PLANNING: &str = "keep_planning";

/// The ACP id of permission mode `mode`; also its `mode` config value.
pub(super) fn mode_id(mode: PermissionMode) -> SessionModeId {
    SessionModeId::new(mode.as_str())
}

pub(super) fn parse_mode_id(id: &str) -> Option<PermissionMode> {
    PermissionMode::ALL
        .into_iter()
        .find(|mode| mode.as_str() == id)
}

/// The tool call card a permission prompt is drawn on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Card<'a> {
    /// The card of the call that asked, which the host already holds.
    Call(&'a wire::ToolCallId),
    /// A fresh card for a call the host never saw.
    Placeholder(String),
}

impl<'a> Card<'a> {
    /// The card for a prompt about `tool_call`. A prompt about no call gets
    /// a placeholder numbered from `counter`, so two never share one.
    pub(super) fn of(tool_call: Option<&'a wire::ToolCallId>, counter: &AtomicU64) -> Self {
        match tool_call {
            Some(id) => Self::Call(id),
            None => Self::Placeholder(format!(
                "pending-{}",
                counter.fetch_add(1, Ordering::Relaxed) + 1
            )),
        }
    }

    fn id(&self) -> ToolCallId {
        match self {
            Self::Call(id) => ToolCallId::new(id.as_str()),
            Self::Placeholder(id) => ToolCallId::new(id.as_str()),
        }
    }
}

/// The prompt for one approval, drawn on `card`. The card of a call the
/// host already saw keeps the kind it was announced with; only a
/// placeholder, which has no kind yet, takes one from the operation.
pub(super) fn permission_request(
    session_id: &SessionId,
    ask: &ApprovalAsk,
    card: &Card<'_>,
) -> RequestPermissionRequest {
    let fields = ToolCallUpdateFields::new().title(ask.reason.clone());
    let fields = match card {
        Card::Call(_) => fields,
        Card::Placeholder(_) => fields.kind(tool_kind_for_operation(&ask.operation)),
    };
    RequestPermissionRequest::new(
        session_id.clone(),
        ToolCallUpdate::new(card.id(), fields),
        permission_options(),
    )
}

/// The daemon's answer to an approval the host answered with `outcome`.
/// A host that cancelled the prompt answers nothing: the turn's cancel
/// withdraws the request.
pub(super) fn approval_answer(outcome: &RequestPermissionOutcome) -> Option<InputAnswer> {
    let selected = match outcome {
        RequestPermissionOutcome::Cancelled => return None,
        RequestPermissionOutcome::Selected(selected) => selected,
        _ => {
            return Some(InputAnswer::Deny {
                reason: Some("unknown permission outcome".into()),
            });
        }
    };
    Some(match selected.option_id.0.as_ref() {
        OPTION_ALLOW_ONCE => InputAnswer::AllowOnce,
        OPTION_ALLOW_ALWAYS => InputAnswer::AllowForSession,
        OPTION_REJECT_ONCE => InputAnswer::Deny { reason: None },
        _ => InputAnswer::Deny {
            reason: Some("unknown permission option".into()),
        },
    })
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

fn tool_kind_for_operation(operation: &Operation) -> ToolKind {
    match operation {
        Operation::ReadPath { .. } | Operation::DiscoverInstructions { .. } => ToolKind::Read,
        Operation::WritePath { .. } => ToolKind::Edit,
        Operation::RunProcess { .. } => ToolKind::Execute,
        Operation::Network { .. } => ToolKind::Fetch,
        Operation::LoadSkill { .. } | Operation::ControlSession { .. } => ToolKind::Other,
        // `Unknown`, or an operation a newer daemon sends.
        _ => ToolKind::Other,
    }
}

/// The plan approval prompt, drawn on `card`, the `exit_plan_mode` call's,
/// with the plan as its content. Each option approves into one mode, by mode id,
/// so hosts show what each choice grants; "keep planning" rejects. A mode
/// this rho does not know is not offered.
pub(super) fn plan_approval_request(
    session_id: &SessionId,
    ask: &PlanApprovalAsk,
    card: &Card<'_>,
) -> RequestPermissionRequest {
    let mut options = ask
        .modes
        .iter()
        .filter_map(|&mode| PermissionMode::try_from(mode).ok())
        .map(|mode| {
            // Hosts style "always" as the broader grant; supervised still
            // asks before each change, so it is the "once" choice.
            let kind = match mode {
                PermissionMode::Supervised => PermissionOptionKind::AllowOnce,
                PermissionMode::AllowEdits | PermissionMode::Auto | PermissionMode::Bypass => {
                    PermissionOptionKind::AllowAlways
                }
                PermissionMode::Plan => PermissionOptionKind::RejectOnce,
            };
            PermissionOption::new(mode.as_str(), plan_exit::approval_label(mode), kind)
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
            card.id(),
            ToolCallUpdateFields::new()
                .title("Plan ready. How should Rho continue?")
                .kind(ToolKind::SwitchMode)
                .content(vec![ToolCallContent::from(ask.plan.clone())]),
        ),
        options,
    )
}

/// The user's plan decision; the daemon checks the mode was offered. Any
/// option other than a mode keeps planning, and a cancelled prompt answers
/// nothing, as an approval's does.
pub(super) fn plan_answer(outcome: &RequestPermissionOutcome) -> Option<InputAnswer> {
    let selected = match outcome {
        RequestPermissionOutcome::Cancelled => return None,
        RequestPermissionOutcome::Selected(selected) => selected,
        _ => return Some(InputAnswer::KeepPlanning { feedback: None }),
    };
    Some(match parse_mode_id(selected.option_id.0.as_ref()) {
        Some(mode) => InputAnswer::ApprovePlan {
            mode: mode.into(),
            feedback: None,
        },
        None => InputAnswer::KeepPlanning { feedback: None },
    })
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
