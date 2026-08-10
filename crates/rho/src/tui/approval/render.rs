use std::path::Path;
use std::time::Duration;

use ratatui::text::Line;
use rho_sdk::{
    CapabilityKind, CapabilityOperation, CapabilityRequest, CapabilitySource, ExecutableSelection,
    PathScope, ProcessEnvironment, ProcessExecution,
};

use super::{ApprovalChoice, ApprovalComposer};
use crate::tui::{
    render::{push_wrapped_text, truncate_one_line, LineFill},
    theme::Theme,
};

/// Fixed composer rows outside the detail window: title, choices, status, footer.
const APPROVAL_FIXED_COMPOSER_ROWS: usize = 1 + ApprovalChoice::ALL.len() + 2;
/// Frame around the composer: minimum history, dividers, and statusline.
const APPROVAL_FRAME_ROWS: usize = 6;
const APPROVAL_DETAIL_CHROME_ROWS: usize = APPROVAL_FIXED_COMPOSER_ROWS + APPROVAL_FRAME_ROWS;
const MIN_DETAIL_PAGE_LINES: usize = 3;
const PRIMARY_INDENT: &str = "  ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailRole {
    Primary,
    Meta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DetailLine {
    text: String,
    role: DetailRole,
}

pub(in crate::tui) fn approval_lines(
    approval: &ApprovalComposer,
    width: usize,
    viewport_height: usize,
) -> Vec<Line<'static>> {
    approval_lines_for_position(
        approval.request().capability(),
        approval.request().reason(),
        approval.active(),
        approval.detail_offset(),
        width,
        viewport_height,
    )
}

pub(super) fn approval_lines_for_position(
    request: &CapabilityRequest,
    reason: &str,
    active: ApprovalChoice,
    detail_offset: usize,
    width: usize,
    viewport_height: usize,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let page_lines = approval_detail_page_lines(viewport_height);
    let mut lines = vec![Line::styled(
        truncate_one_line(&approval_title(request), width),
        Theme::input_prompt(),
    )];

    let details = wrapped_detail_lines(request, reason, width);
    let max_offset = details.len().saturating_sub(page_lines);
    let detail_offset = detail_offset.min(max_offset);
    let detail_end = (detail_offset + page_lines).min(details.len());
    lines.extend(details[detail_offset..detail_end].iter().cloned());

    for choice in ApprovalChoice::ALL {
        let selected = choice == active;
        lines.push(Line::styled(
            truncate_one_line(
                &format!(
                    "{} {}",
                    if selected {
                        crate::tui::composer_chrome::SELECTION_MARKER_ACTIVE
                    } else {
                        crate::tui::composer_chrome::SELECTION_MARKER_INACTIVE
                    },
                    choice.label()
                ),
                width,
            ),
            if selected {
                Theme::input_prompt()
            } else {
                Theme::dim()
            },
        ));
    }

    if details.len() > page_lines {
        let earlier = if detail_offset > 0 {
            " · ↑ earlier"
        } else {
            ""
        };
        let later = if detail_end < details.len() {
            " · ↓ later"
        } else {
            ""
        };
        lines.push(Line::styled(
            truncate_one_line(
                &format!(
                    "PgUp/PgDn details {}-{}/{}{}{}",
                    detail_offset + 1,
                    detail_end,
                    details.len(),
                    earlier,
                    later
                ),
                width,
            ),
            Theme::dim(),
        ));
    } else {
        // Keep a stable slot so choice rows do not jump when paging appears.
        lines.push(Line::styled(String::new(), Theme::dim()));
    }
    lines.push(Line::styled(
        truncate_one_line(
            &crate::tui::composer_chrome::join_footer_parts([
                "Enter confirm",
                "arrows choose",
                "Esc deny & cancel",
            ]),
            width,
        ),
        Theme::dim(),
    ));
    lines
}

pub(super) fn approval_detail_line_count(
    request: &rho_sdk::ApprovalRequest,
    width: usize,
) -> usize {
    wrapped_detail_lines(request.capability(), request.reason(), width.max(1)).len()
}

pub(super) fn approval_detail_page_lines(viewport_height: usize) -> usize {
    viewport_height
        .saturating_sub(APPROVAL_DETAIL_CHROME_ROWS)
        .max(MIN_DETAIL_PAGE_LINES)
}

fn wrapped_detail_lines(
    request: &CapabilityRequest,
    reason: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for detail in approval_detail_lines(request) {
        let style = match detail.role {
            DetailRole::Primary => Theme::text(),
            DetailRole::Meta => Theme::dim(),
        };
        push_wrapped_text(&mut lines, &detail.text, width, style, LineFill::Natural);
    }
    let reason = reason.trim();
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
        CapabilityKind::Process => "run a command",
        CapabilityKind::Read => "read",
        CapabilityKind::Network => "access the network",
        CapabilityKind::Skill => "load a skill",
        CapabilityKind::InstructionDiscovery => "discover instructions",
        _ => "use a capability",
    };
    format!("{} wants to {verb}", sanitize_controls(tool))
}

/// Flattened detail strings for security and layout tests.
#[cfg(test)]
pub(super) fn approval_details(request: &CapabilityRequest) -> Vec<String> {
    approval_detail_lines(request)
        .into_iter()
        .map(|detail| detail.text)
        .collect()
}

fn approval_detail_lines(request: &CapabilityRequest) -> Vec<DetailLine> {
    match request.operation() {
        CapabilityOperation::ReadPath { path, scope }
        | CapabilityOperation::WritePath { path, scope }
        | CapabilityOperation::DiscoverInstructions { path, scope } => vec![
            primary(sanitize_controls(&path.to_string_lossy())),
            meta(format_path_scope(scope)),
        ],
        CapabilityOperation::ExecuteProcess(execution) => process_details(execution),
        CapabilityOperation::NetworkAccess(target) => vec![primary(format!(
            "target: {}",
            sanitize_controls(target.url().unwrap_or("tool-managed network access"))
        ))],
        CapabilityOperation::LoadSkill { name, path } => {
            let mut details = vec![primary(format!("skill: {}", sanitize_controls(name)))];
            if let Some(path) = path {
                details.push(meta(format!(
                    "path: {}",
                    sanitize_controls(&path.to_string_lossy())
                )));
            }
            details
        }
        _ => Vec::new(),
    }
}

fn process_details(execution: &ProcessExecution) -> Vec<DetailLine> {
    let invocation = execution.invocation();
    let mut details = Vec::new();

    if let Some(command) = invocation.shell_command() {
        details.push(primary(format!(
            "{PRIMARY_INDENT}{}",
            sanitize_controls(command)
        )));
    } else {
        details.push(primary(format!(
            "{PRIMARY_INDENT}{}",
            format_direct_invocation(invocation.executable_path(), invocation.arguments())
        )));
    }

    details.push(meta(format!(
        "cwd  {}",
        sanitize_controls(&execution.working_directory().to_string_lossy())
    )));
    details.push(meta(format_process_context(execution)));
    details
}

fn format_process_context(execution: &ProcessExecution) -> String {
    let invocation = execution.invocation();
    let mut parts = Vec::new();

    if invocation.shell_command().is_some() {
        let executable = sanitize_controls(&invocation.executable_path().to_string_lossy());
        let args = invocation
            .arguments()
            .iter()
            .map(|argument| sanitize_controls(argument))
            .collect::<Vec<_>>()
            .join(" ");
        let via = if args.is_empty() {
            executable
        } else {
            format!("{executable} {args}")
        };
        let lookup = match invocation.executable_selection() {
            ExecutableSelection::ExactPath => "exact path",
            ExecutableSelection::SearchPath => "PATH",
            _ => "unspecified",
        };
        parts.push(format!("via {via} ({lookup})"));
    } else {
        let lookup = match invocation.executable_selection() {
            ExecutableSelection::ExactPath => "exact path",
            ExecutableSelection::SearchPath => "PATH",
            _ => "unspecified",
        };
        parts.push(lookup.into());
    }

    parts.push(format_environment_summary(execution.environment()));

    let limits = execution.output_limits();
    parts.push(format!(
        "{} out",
        format_byte_count(limits.max_output_bytes())
    ));
    parts.push(format_timeout(limits.timeout()));

    parts.join(" · ")
}

fn format_path_scope(scope: &PathScope) -> String {
    match scope {
        PathScope::PrimaryWorkspace => "scope: primary workspace".into(),
        PathScope::GrantedRoot { root } => format!(
            "scope: granted root {}",
            sanitize_controls(&root.to_string_lossy())
        ),
        PathScope::UnrestrictedFilesystem => "scope: unrestricted filesystem".into(),
        _ => "scope: unspecified".into(),
    }
}

fn format_environment_summary(environment: &ProcessEnvironment) -> String {
    match environment {
        ProcessEnvironment::Empty => "env empty".into(),
        ProcessEnvironment::InheritAll => "env inherit all".into(),
        ProcessEnvironment::InheritExcept { variable_names } => {
            let count = variable_names.len();
            if count == 1 {
                "env inherit (1 var stripped)".into()
            } else {
                format!("env inherit ({count} vars stripped)")
            }
        }
        ProcessEnvironment::InheritListed { variable_names } => {
            let count = variable_names.len();
            if count == 1 {
                "env inherit 1 listed var".into()
            } else {
                format!("env inherit {count} listed vars")
            }
        }
        _ => "env unspecified".into(),
    }
}

fn format_timeout(timeout: Option<Duration>) -> String {
    match timeout {
        None => "no timeout".into(),
        Some(timeout) => format!("timeout {timeout:?}"),
    }
}

fn format_byte_count(bytes: usize) -> String {
    const KB: usize = 1000;
    const MB: usize = 1000 * 1000;
    if bytes >= MB && bytes.is_multiple_of(MB) {
        format!("{} MB", bytes / MB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB && bytes.is_multiple_of(KB) {
        format!("{} KB", bytes / KB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn primary(text: impl Into<String>) -> DetailLine {
    DetailLine {
        text: text.into(),
        role: DetailRole::Primary,
    }
}

fn meta(text: impl Into<String>) -> DetailLine {
    DetailLine {
        text: text.into(),
        role: DetailRole::Meta,
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
