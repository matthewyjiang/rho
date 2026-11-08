//! A daemon session's approval ask as the approval prompt the in-process
//! TUI shows (#1667): the same title, details, and reason, drawn from the
//! wire's operation instead of the runtime's capability request.
//!
//! The wire carries what a person decides on, not the runtime's whole
//! request: a process's environment, output limit, and timeout stay in the
//! daemon, so its details show only what the ask says rather than guess.

use rho_protocol::{ApprovalAsk, ApprovalSource, Operation, PathScope as WirePathScope};
use rho_sdk::{CapabilityKind, PathScope};

use super::render::{
    format_direct_invocation, format_path_scope, sanitize_controls, title, Actor, ApprovalDetails,
    ApprovalPrompt,
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
        // `Unknown`, or an operation a newer daemon sends: the title says
        // this rho cannot name it.
        _ => ApprovalDetails::default(),
    }
}

/// Where a path lies, as the in-process prompt says it.
fn path_scope(scope: &WirePathScope) -> String {
    let scope = match scope {
        WirePathScope::Workspace => PathScope::PrimaryWorkspace,
        WirePathScope::GrantedRoot { root } => PathScope::GrantedRoot { root: root.clone() },
        WirePathScope::Unrestricted => PathScope::UnrestrictedFilesystem,
        // `Unknown`, or a scope a newer daemon sends.
        _ => return "scope unspecified".into(),
    };
    format_path_scope(&scope)
}
