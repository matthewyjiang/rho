use std::path::Path;

use ratatui::text::Line;
use rho_sdk::{
    CapabilityKind, CapabilityOperation, CapabilityRequest, CapabilitySource, ExecutableSelection,
    PathScope, ProcessEnvironment, ProcessExecution,
};

use super::ApprovalComposer;
use crate::tui::{
    render::{push_wrapped_text, truncate_one_line, LineFill},
    theme::Theme,
};

const CHOICES: [&str; 3] = ["Allow once", "Allow for session", "Deny"];
const DETAIL_PAGE_LINES: usize = 3;

pub(in crate::tui) fn approval_lines(
    approval: &ApprovalComposer,
    width: usize,
) -> Vec<Line<'static>> {
    approval_lines_for_position(
        approval.request().capability(),
        approval.request().reason(),
        approval.active(),
        approval.detail_pages_before_end(),
        width,
    )
}

pub(super) fn approval_lines_for_position(
    request: &CapabilityRequest,
    reason: &str,
    active: usize,
    detail_pages_before_end: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = vec![Line::styled(
        truncate_one_line(&approval_title(request), width),
        Theme::input_prompt(),
    )];

    let details = wrapped_detail_lines(request, reason, width);
    let detail_start = details
        .len()
        .saturating_sub(DETAIL_PAGE_LINES.saturating_mul(detail_pages_before_end + 1));
    let detail_end = (detail_start + DETAIL_PAGE_LINES).min(details.len());
    lines.extend(details[detail_start..detail_end].iter().cloned());

    for (index, choice) in CHOICES.iter().enumerate() {
        let selected = index == active;
        lines.push(Line::styled(
            truncate_one_line(
                &format!("{} {choice}", if selected { ">" } else { " " }),
                width,
            ),
            if selected {
                Theme::input_prompt()
            } else {
                Theme::dim()
            },
        ));
    }

    let detail_status = if details.len() > DETAIL_PAGE_LINES {
        let earlier = if detail_start > 0 {
            " · ↑ earlier"
        } else {
            ""
        };
        let later = if detail_end < details.len() {
            " · ↓ later"
        } else {
            ""
        };
        format!(
            "pgup/pgdn details {}-{}/{}{}{}",
            detail_start + 1,
            detail_end,
            details.len(),
            earlier,
            later
        )
    } else {
        format!("details 1-{}/{}", detail_end, details.len())
    };
    lines.push(Line::styled(
        truncate_one_line(&detail_status, width),
        Theme::dim(),
    ));
    lines.push(Line::styled(
        truncate_one_line("enter confirm · arrows choose · esc deny & cancel", width),
        Theme::dim(),
    ));
    lines
}

pub(super) fn approval_detail_page_count(
    request: &rho_sdk::ApprovalRequest,
    width: usize,
) -> usize {
    wrapped_detail_lines(request.capability(), request.reason(), width.max(1))
        .len()
        .div_ceil(DETAIL_PAGE_LINES)
        .max(1)
}

fn wrapped_detail_lines(
    request: &CapabilityRequest,
    reason: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_wrapped_text(
        &mut lines,
        source_detail(request.source()),
        width,
        Theme::dim(),
        LineFill::Natural,
    );
    for detail in approval_details(request) {
        push_wrapped_text(&mut lines, &detail, width, Theme::text(), LineFill::Natural);
    }
    if !reason.is_empty() {
        push_wrapped_text(
            &mut lines,
            &format!("reason: {}", sanitize_controls(reason)),
            width,
            Theme::dim(),
            LineFill::Natural,
        );
    }
    lines
}

pub(super) fn approval_title(request: &CapabilityRequest) -> String {
    let tool = match request.source() {
        CapabilitySource::BuiltInTool { name } | CapabilitySource::HostProvidedTool { name } => {
            name.as_str()
        }
        CapabilitySource::PromptConstruction => "rho",
        _ => "rho",
    };
    let verb = match request.kind() {
        CapabilityKind::Write => "write",
        CapabilityKind::Process => "execute",
        CapabilityKind::Read => "read",
        CapabilityKind::Network => "access the network",
        CapabilityKind::Skill => "load a skill",
        CapabilityKind::InstructionDiscovery => "discover instructions",
        _ => "use a capability",
    };
    format!("{} wants to {verb}", sanitize_controls(tool))
}

pub(super) fn approval_details(request: &CapabilityRequest) -> Vec<String> {
    match request.operation() {
        CapabilityOperation::ReadPath { path, scope }
        | CapabilityOperation::WritePath { path, scope }
        | CapabilityOperation::DiscoverInstructions { path, scope } => vec![
            format!("path: {}", sanitize_controls(&path.to_string_lossy())),
            format_path_scope(scope),
        ],
        CapabilityOperation::ExecuteProcess(execution) => process_details(execution),
        CapabilityOperation::NetworkAccess(target) => vec![format!(
            "target: {}",
            sanitize_controls(target.url().unwrap_or("tool-managed network access"))
        )],
        CapabilityOperation::LoadSkill { name, path } => {
            let mut details = vec![format!("skill: {}", sanitize_controls(name))];
            if let Some(path) = path {
                details.push(format!(
                    "path: {}",
                    sanitize_controls(&path.to_string_lossy())
                ));
            }
            details
        }
        _ => Vec::new(),
    }
}

fn process_details(execution: &ProcessExecution) -> Vec<String> {
    let invocation = execution.invocation();
    let mut details = vec![format!(
        "working directory: {}",
        sanitize_controls(&execution.working_directory().to_string_lossy())
    )];
    let invocation_display =
        format_direct_invocation(invocation.executable_path(), invocation.arguments());
    details.push(format!(
        "executable resolution: {}",
        match invocation.executable_selection() {
            ExecutableSelection::ExactPath => "exact path",
            ExecutableSelection::SearchPath => "PATH search",
            _ => "unspecified",
        }
    ));
    details.push(format_environment(execution.environment()));

    let limits = execution.output_limits();
    details.push(format!(
        "output limit: {} bytes; timeout: {}",
        limits.max_output_bytes(),
        limits
            .timeout()
            .map_or_else(|| "none".into(), |timeout| format!("{timeout:?}"),)
    ));
    if let Some(command) = invocation.shell_command() {
        details.push(format!(
            "shell invocation (JSON-style args): {invocation_display}"
        ));
        details.push(format!("command: {}", sanitize_controls(command)));
    } else {
        details.push(format!(
            "invocation (JSON-style args): {invocation_display}"
        ));
    }
    details
}

fn format_path_scope(scope: &PathScope) -> String {
    match scope {
        PathScope::PrimaryWorkspace => "scope: primary workspace".into(),
        PathScope::GrantedRoot { root } => format!(
            "scope: granted root {}",
            sanitize_controls(&root.to_string_lossy())
        ),
        _ => "scope: unspecified".into(),
    }
}

fn format_environment(environment: &ProcessEnvironment) -> String {
    match environment {
        ProcessEnvironment::Empty => "environment: empty".into(),
        ProcessEnvironment::InheritAll => "environment: inherit all variables".into(),
        ProcessEnvironment::InheritExcept { variable_names } => format!(
            "environment: inherit all except {}",
            format_json_strings(variable_names.iter().map(String::as_str))
        ),
        ProcessEnvironment::InheritListed { variable_names } => format!(
            "environment: inherit listed variable names {}",
            format_json_strings(variable_names.iter().map(String::as_str))
        ),
        _ => "environment: unspecified".into(),
    }
}

fn source_detail(source: &CapabilitySource) -> &'static str {
    match source {
        CapabilitySource::BuiltInTool { .. } => "source: built-in tool",
        CapabilitySource::HostProvidedTool { .. } => "source: host-provided tool",
        CapabilitySource::PromptConstruction => "source: rho prompt construction",
        _ => "source: unspecified",
    }
}

/// Formats the executable and arguments as a JSON-style array for display.
/// This makes argument boundaries explicit without presenting a shell command.
pub(super) fn format_direct_invocation(executable: &Path, arguments: &[String]) -> String {
    let executable = executable.to_string_lossy();
    format_json_strings(
        std::iter::once(executable.as_ref()).chain(arguments.iter().map(String::as_str)),
    )
}

fn format_json_strings<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let serialized = values
        .into_iter()
        .map(|value| serde_json::to_string(value).expect("strings always serialize as JSON"))
        .collect::<Vec<_>>()
        .join(", ");
    sanitize_controls(&format!("[{serialized}]"))
}

fn sanitize_controls(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' => "\\n".into(),
            '\r' => "\\r".into(),
            '\t' => "\\t".into(),
            ch if ch.is_control() || is_unicode_format_control(ch) => {
                format!("\\u{{{:x}}}", ch as u32)
            }
            ch => ch.to_string(),
        })
        .collect()
}

fn is_unicode_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}
