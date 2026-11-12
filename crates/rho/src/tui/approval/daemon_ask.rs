//! A daemon session's approval ask as the approval prompt: its title,
//! details, and reason, drawn from the wire's operation.
//!
//! The wire carries what a person decides on, not the runtime's whole
//! request: a process's environment, output limit, and timeout stay in the
//! daemon, so its details show only what the ask says rather than guess.

use rho_protocol::{ApprovalAsk, ApprovalSource, Operation, PathScope};
use rho_sdk::CapabilityKind;

use super::render::{
    format_direct_invocation, sanitize_controls, session_control_details, title, Actor,
    ApprovalDetails, ApprovalPrompt,
};

impl ApprovalPrompt {
    /// The prompt for a daemon session's approval ask.
    pub(in crate::tui) fn of_daemon_ask(ask: &ApprovalAsk) -> Self {
        let actor = match &ask.source {
            ApprovalSource::BuiltInTool { name } => Actor::BuiltInTool(name),
            ApprovalSource::HostTool { name } => Actor::HostTool(name),
            ApprovalSource::Rho => Actor::Rho,
            // `Unknown`, or a source a newer daemon sends.
            _ => Actor::Rho,
        };
        Self {
            title: title(actor, kind(&ask.operation)),
            details: details(&ask.operation),
            reason: ask.reason.clone(),
        }
    }
}

/// The capability `operation` asks for; `None` for one this rho cannot name.
fn kind(operation: &Operation) -> Option<CapabilityKind> {
    match operation {
        Operation::ReadPath { .. } => Some(CapabilityKind::Read),
        Operation::WritePath { .. } => Some(CapabilityKind::Write),
        Operation::DiscoverInstructions { .. } => Some(CapabilityKind::InstructionDiscovery),
        Operation::RunProcess { .. } => Some(CapabilityKind::Process),
        Operation::Network { .. } => Some(CapabilityKind::Network),
        Operation::LoadSkill { .. } => Some(CapabilityKind::Skill),
        Operation::ControlSession { .. } => Some(CapabilityKind::SessionControl),
        // `Unknown`, or an operation a newer daemon sends.
        _ => None,
    }
}

fn details(operation: &Operation) -> ApprovalDetails {
    match operation {
        Operation::ReadPath { path, scope }
        | Operation::WritePath { path, scope }
        | Operation::DiscoverInstructions { path, scope } => ApprovalDetails {
            primary: vec![sanitize_controls(&path.to_string_lossy())],
            meta: vec![path_scope(scope)],
        },
        Operation::RunProcess {
            program,
            args,
            shell_command,
            cwd,
        } => {
            let direct = format_direct_invocation(program, args);
            let mut meta = vec![format!("cwd {}", sanitize_controls(&cwd.to_string_lossy()))];
            let primary = match shell_command {
                Some(command) => {
                    meta.push(format!("via {direct}"));
                    sanitize_controls(command)
                }
                None => direct,
            };
            ApprovalDetails {
                primary: vec![primary],
                meta: vec![meta.join(" · ")],
            }
        }
        Operation::Network { url } => ApprovalDetails {
            primary: vec![sanitize_controls(
                url.as_deref().unwrap_or("tool-managed network access"),
            )],
            meta: Vec::new(),
        },
        Operation::LoadSkill { name, path } => ApprovalDetails {
            primary: vec![sanitize_controls(name)],
            meta: path
                .iter()
                .map(|path| format!("path {}", sanitize_controls(&path.to_string_lossy())))
                .collect(),
        },
        Operation::ControlSession {
            action,
            session,
            workspace,
            text,
        } => session_control_details(
            action.as_str(),
            session.as_ref().map(rho_protocol::SessionId::as_str),
            workspace,
            text.as_deref(),
        ),
        // `Unknown`, or an operation a newer daemon sends: the title says
        // this rho cannot name it.
        _ => ApprovalDetails::default(),
    }
}

/// Where a path lies.
fn path_scope(scope: &PathScope) -> String {
    match scope {
        PathScope::Workspace => "scope primary workspace".into(),
        PathScope::GrantedRoot { root } => format!(
            "scope granted root {}",
            sanitize_controls(&root.to_string_lossy())
        ),
        PathScope::Unrestricted => "scope unrestricted filesystem".into(),
        // `Unknown`, or a scope a newer daemon sends.
        _ => "scope unspecified".into(),
    }
}

#[cfg(test)]
#[path = "daemon_ask_tests.rs"]
mod tests;
