//! How the daemon shows a turn's input requests to clients (#1660): the
//! runtime's approval, plan-approval, and question requests as the wire's
//! [`Ask`]s, and the permission modes a plan approval names.

use std::path::{Path, PathBuf};

use rho_protocol::{
    self as wire, ApprovalAsk, ApprovalSource, Ask, Choice, DefaultAnswer, DefaultStyle, Operation,
    PlanApprovalAsk, Question, QuestionAsk, Selection,
};
use rho_sdk::{
    ApprovalRequest, CapabilityOperation, CapabilitySource, DefaultSelection, HostInputRequest,
    HostQuestion, PathScope, SelectionMode,
};
use serde_json::Value;

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
            path: wire_path(path),
            scope: path_scope(scope),
        },
        CapabilityOperation::WritePath { path, scope } => Operation::WritePath {
            path: wire_path(path),
            scope: path_scope(scope),
        },
        CapabilityOperation::DiscoverInstructions { path, scope } => {
            Operation::DiscoverInstructions {
                path: wire_path(path),
                scope: path_scope(scope),
            }
        }
        CapabilityOperation::ExecuteProcess(execution) => {
            let invocation = execution.invocation();
            Operation::RunProcess {
                program: wire_path(invocation.executable_path()),
                args: invocation.arguments().to_vec(),
                shell_command: invocation.shell_command().map(str::to_owned),
                cwd: wire_path(execution.working_directory()),
            }
        }
        CapabilityOperation::NetworkAccess(target) => Operation::Network {
            url: target.url().map(str::to_owned),
        },
        CapabilityOperation::LoadSkill { name, path } => Operation::LoadSkill {
            name: name.clone(),
            path: path.as_deref().map(wire_path),
        },
        _ => Operation::Unknown,
    };
    Ask::Approval(ApprovalAsk {
        source,
        operation,
        reason: request.reason().to_owned(),
    })
}

/// `path` as JSON can carry it: the wire's paths are for people to read,
/// and one that is not UTF-8 would keep the whole request from clients.
fn wire_path(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().into_owned())
}

fn path_scope(scope: &PathScope) -> wire::PathScope {
    match scope {
        PathScope::PrimaryWorkspace => wire::PathScope::Workspace,
        PathScope::GrantedRoot { root } => wire::PathScope::GrantedRoot {
            root: wire_path(root),
        },
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

/// The wire timeout of a question whose agent offered fallback answers,
/// when the daemon answers it with them at `deadline_ms`; `None` for a
/// question without.
pub(super) fn timeout(request: &HostInputRequest, deadline_ms: u64) -> Option<wire::InputTimeout> {
    let (fallback, reason) = request.timeout_fallback().zip(request.timeout_reason())?;
    Some(wire::InputTimeout {
        deadline_ms,
        answer: wire::InputAnswer::Answers {
            answers: fallback.answers().clone(),
        },
        reason: reason.to_owned(),
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
        selection: match question.selection() {
            SelectionMode::One => Selection::One,
            SelectionMode::Many => Selection::Many,
            _ => Selection::Unknown,
        },
        allow_other: question.permits_other(),
        help: question.help().map(str::to_owned),
        default: default_answer(question),
        required: question.is_required(),
    }
}

/// The answer `question` suggests, if any. The agent gives it as JSON: a
/// choice's value or label, or a list of them, which this reads as the
/// values an answer would send, keeping text that names no choice as is.
fn default_answer(question: &HostQuestion) -> Option<DefaultAnswer> {
    let values: Vec<String> = match question.default_value()? {
        Value::Array(values) => values.iter().filter_map(scalar_text).collect(),
        value => scalar_text(value).into_iter().collect(),
    };
    let values: Vec<String> = values
        .into_iter()
        .map(|text| {
            question
                .choices()
                .iter()
                .find(|choice| choice.value() == text || choice.label() == text)
                .map_or(text, |choice| choice.value().to_owned())
        })
        .collect();
    if values.is_empty() {
        return None;
    }
    let style = match question.default_selection() {
        DefaultSelection::Selected => DefaultStyle::Selected,
        DefaultSelection::Focused => DefaultStyle::Focused,
        _ => DefaultStyle::Unknown,
    };
    Some(DefaultAnswer { values, style })
}

/// `value` as answer text, if it is a non-empty string, a number, or a bool.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::String(_) | Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn mode_to_wire(mode: PermissionMode) -> wire::PermissionMode {
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

#[cfg(test)]
#[path = "asks_tests.rs"]
mod tests;
