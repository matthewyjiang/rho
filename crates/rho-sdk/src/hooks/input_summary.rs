//! One-line summaries of what a run waits for a person to answer, for the
//! `input_requested` event (#1660). A notification shows one line, so text
//! that can span lines keeps its first non-empty line, and control
//! characters, newlines in paths included, become spaces.

use crate::{
    host_input::HostInputRequest,
    workspace::{ApprovalRequest, CapabilityOperation},
};

/// What an approval asks to do, then its reason when it has one, for
/// example `write /repo/note.txt` or `run git push (outside the workspace)`.
///
/// Mode policies leave the reason empty for ordinary writes and commands, so
/// the operation is what names the request. A network target names only its
/// host: userinfo and query strings routinely carry tokens.
pub(super) fn approval(request: &ApprovalRequest) -> String {
    let operation = match request.capability().operation() {
        CapabilityOperation::ReadPath { path, .. } => format!("read {}", path.display()),
        CapabilityOperation::WritePath { path, .. } => format!("write {}", path.display()),
        CapabilityOperation::DiscoverInstructions { path, .. } => {
            format!("discover instructions in {}", path.display())
        }
        CapabilityOperation::ExecuteProcess(execution) => {
            let invocation = execution.invocation();
            match invocation.shell_command() {
                Some(command) => format!("run {}", first_line(command)),
                None => {
                    let mut command = invocation.executable_path().display().to_string();
                    for argument in invocation.arguments() {
                        command.push(' ');
                        command.push_str(argument);
                    }
                    format!("run {}", first_line(&command))
                }
            }
        }
        CapabilityOperation::NetworkAccess(target) => {
            match target
                .url()
                .and_then(|url| url::Url::parse(url).ok())
                .and_then(|url| url.host_str().map(str::to_owned))
            {
                Some(host) => format!("access {host}"),
                None => "access the network".into(),
            }
        }
        CapabilityOperation::LoadSkill { name, .. } => format!("load skill {name}"),
        CapabilityOperation::ControlSession(target) => match &target.session {
            Some(session) => format!(
                "{} session {} in {}",
                target.action.label(),
                session.as_str(),
                target.workspace.display()
            ),
            None => format!(
                "{} session in {}",
                target.action.label(),
                target.workspace.display()
            ),
        },
    };
    let summary = match first_line(request.reason()) {
        "" => operation,
        reason => format!("{operation} ({reason})"),
    };
    one_line(&summary)
}

/// A questionnaire's title, or its first question's prompt when untitled.
pub(super) fn question(request: &HostInputRequest) -> String {
    let summary = match first_line(request.title()) {
        "" => request
            .questions()
            .first()
            .map_or("", |question| first_line(question.prompt())),
        title => title,
    };
    one_line(summary)
}

/// A plan's first line, usually its heading.
pub(super) fn plan(plan: &str) -> String {
    one_line(first_line(plan))
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "input_summary_tests.rs"]
mod tests;
