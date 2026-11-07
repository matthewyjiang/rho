//! How the daemon shows a turn's input requests to clients (#1660): the
//! runtime's approval, plan-approval, and question requests as the wire's
//! [`Ask`]s, and the permission modes a plan approval names.

use rho_protocol::{
    self as wire, ApprovalAsk, ApprovalSource, Ask, Choice, Operation, PlanApprovalAsk, Question,
    QuestionAsk,
};
use rho_sdk::{
    ApprovalRequest, CapabilityOperation, CapabilitySource, DefaultSelection, HostInputRequest,
    HostQuestion, PathScope, SelectionMode,
};

use crate::{app::session_runtime::PlanApprovalRequest, permission::PermissionMode};

/// The wire ask for a tool approval.
pub(super) fn approval(request: &ApprovalRequest) -> Ask {
    let capability = request.capability();
    let source = match capability.source() {
        CapabilitySource::BuiltInTool { name } => {
            ApprovalSource::BuiltInTool { name: name.clone() }
        }
        CapabilitySource::HostProvidedTool { name } => {
            ApprovalSource::HostTool { name: name.clone() }
        }
        CapabilitySource::PromptConstruction => ApprovalSource::Rho,
        _ => ApprovalSource::Unknown,
    };
    let operation = match capability.operation() {
        CapabilityOperation::ReadPath { path, scope } => Operation::ReadPath {
            path: path.clone(),
            scope: path_scope(scope),
        },
        CapabilityOperation::WritePath { path, scope } => Operation::WritePath {
            path: path.clone(),
            scope: path_scope(scope),
        },
        CapabilityOperation::DiscoverInstructions { path, scope } => {
            Operation::DiscoverInstructions {
                path: path.clone(),
                scope: path_scope(scope),
            }
        }
        CapabilityOperation::ExecuteProcess(execution) => {
            let invocation = execution.invocation();
            Operation::RunProcess {
                program: invocation.executable_path().to_path_buf(),
                args: invocation.arguments().to_vec(),
                shell_command: invocation.shell_command().map(str::to_owned),
                cwd: execution.working_directory().to_path_buf(),
            }
        }
        CapabilityOperation::NetworkAccess(target) => Operation::Network {
            url: target.url().map(str::to_owned),
        },
        CapabilityOperation::LoadSkill { name, path } => Operation::LoadSkill {
            name: name.clone(),
            path: path.clone(),
        },
        _ => Operation::Unknown,
    };
    Ask::Approval(ApprovalAsk {
        source,
        operation,
        reason: request.reason().to_owned(),
        tool_call: request
            .tool_call_id()
            .map(|id| wire::ToolCallId::new(id.as_str())),
    })
}

fn path_scope(scope: &PathScope) -> wire::PathScope {
    match scope {
        PathScope::PrimaryWorkspace => wire::PathScope::Workspace,
        PathScope::GrantedRoot { root } => wire::PathScope::GrantedRoot { root: root.clone() },
        PathScope::UnrestrictedFilesystem => wire::PathScope::Unrestricted,
        _ => wire::PathScope::Unknown,
    }
}

/// The wire ask for a plan approval.
pub(super) fn plan_approval(request: &PlanApprovalRequest) -> Ask {
    Ask::PlanApproval(PlanApprovalAsk {
        plan: request.plan.clone(),
        modes: request.targets.iter().copied().map(mode_to_wire).collect(),
    })
}

/// The wire ask for a questionnaire.
pub(super) fn question(request: &HostInputRequest) -> Ask {
    Ask::Question(QuestionAsk {
        title: request.title().to_owned(),
        questions: request.questions().iter().map(question_to_wire).collect(),
    })
}

fn question_to_wire(question: &HostQuestion) -> Question {
    Question {
        id: question.id().to_owned(),
        prompt: question.prompt().to_owned(),
        header: question.header().map(str::to_owned),
        choices: question
            .choices()
            .iter()
            .map(|choice| Choice {
                value: choice.value().to_owned(),
                label: choice.label().to_owned(),
                description: choice.description().map(str::to_owned),
            })
            .collect(),
        multiple: !matches!(question.selection(), SelectionMode::One),
        allow_other: question.permits_other(),
        help: question.help().map(str::to_owned),
        default: question.default_value().cloned(),
        preselect_default: !matches!(question.default_selection(), DefaultSelection::Focused),
        required: question.is_required(),
    }
}

pub(super) fn mode_to_wire(mode: PermissionMode) -> wire::PermissionMode {
    match mode {
        PermissionMode::Bypass => wire::PermissionMode::Bypass,
        PermissionMode::Auto => wire::PermissionMode::Auto,
        PermissionMode::AllowEdits => wire::PermissionMode::AllowEdits,
        PermissionMode::Plan => wire::PermissionMode::Plan,
        PermissionMode::Supervised => wire::PermissionMode::Supervised,
    }
}

/// The mode a client named, or `None` for one this daemon does not know.
pub(super) fn mode_from_wire(mode: wire::PermissionMode) -> Option<PermissionMode> {
    match mode {
        wire::PermissionMode::Bypass => Some(PermissionMode::Bypass),
        wire::PermissionMode::Auto => Some(PermissionMode::Auto),
        wire::PermissionMode::AllowEdits => Some(PermissionMode::AllowEdits),
        wire::PermissionMode::Plan => Some(PermissionMode::Plan),
        wire::PermissionMode::Supervised => Some(PermissionMode::Supervised),
        _ => None,
    }
}
