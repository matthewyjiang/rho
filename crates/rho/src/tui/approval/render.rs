use std::path::Path;
use std::time::Duration;

use ratatui::text::Line;
use rho_sdk::{
    CapabilityKind, CapabilityOperation, CapabilityRequest, CapabilitySource, ExecutableSelection,
    PathScope, ProcessEnvironment, ProcessExecution,
};

use super::{ApprovalChoice, ApprovalComposer};
use crate::{
    control_chars::{escape_controls, LineBreaks},
    tui::{
        composer_pointer::ComposerHit,
        render::{push_wrapped_text, truncate_one_line, LineFill},
        theme::Theme,
    },
};

/// Fixed composer rows outside the detail window: title, choices, status, footer.
const APPROVAL_FIXED_COMPOSER_ROWS: usize = 1 + ApprovalChoice::ALL.len() + 2;
/// Frame around the composer: minimum history, dividers, and statusline.
const APPROVAL_FRAME_ROWS: usize = 6;
const APPROVAL_DETAIL_CHROME_ROWS: usize = APPROVAL_FIXED_COMPOSER_ROWS + APPROVAL_FRAME_ROWS;
const MIN_DETAIL_PAGE_LINES: usize = 3;

/// Body rows for an approval prompt: the thing being approved, then dim context.
#[derive(Clone, Debug, Default)]
pub(super) struct ApprovalDetails {
    pub(super) primary: Vec<String>,
    pub(super) meta: Vec<String>,
}

/// What an approval prompt shows, whoever asks: its title, the details of
/// what it would allow, and the policy's reason. Built once when the prompt
/// opens, from the in-process runtime's request or a daemon session's ask
/// (`daemon_ask`).
#[derive(Clone, Debug)]
pub(in crate::tui) struct ApprovalPrompt {
    pub(super) title: String,
    pub(super) details: ApprovalDetails,
    pub(super) reason: String,
}

impl ApprovalPrompt {
    /// The prompt for an in-process capability request.
    pub(in crate::tui) fn of_capability(request: &CapabilityRequest, reason: &str) -> Self {
        Self {
            title: approval_title(request),
            details: approval_detail_sections(request),
            reason: reason.to_owned(),
        }
    }
}

/// Approval composer rows plus the clickable choice rows.
pub(in crate::tui) struct ApprovalFrame {
    pub(in crate::tui) lines: Vec<Line<'static>>,
    pub(in crate::tui) hits: Vec<ComposerHit<ApprovalChoice>>,
}

pub(in crate::tui) fn approval_frame(
    approval: &ApprovalComposer,
    width: usize,
    viewport_height: usize,
) -> ApprovalFrame {
    approval_frame_for_position(
        approval.prompt(),
        approval.active(),
        approval.detail_offset(),
        width,
        viewport_height,
    )
}

pub(super) fn approval_frame_for_position(
    prompt: &ApprovalPrompt,
    active: ApprovalChoice,
    detail_offset: usize,
    width: usize,
    viewport_height: usize,
) -> ApprovalFrame {
    let width = width.max(1);
    let page_lines = approval_detail_page_lines(viewport_height);
    let mut lines = vec![Line::styled(
        truncate_one_line(&prompt.title, width),
        Theme::input_prompt(),
    )];

    let details = wrapped_detail_lines(prompt, width);
    let max_offset = details.len().saturating_sub(page_lines);
    let detail_offset = detail_offset.min(max_offset);
    let detail_end = (detail_offset + page_lines).min(details.len());
    lines.extend(details[detail_offset..detail_end].iter().cloned());

    let mut hits = Vec::with_capacity(ApprovalChoice::ALL.len());
    for choice in ApprovalChoice::ALL {
        let selected = choice == active;
        let hit = ComposerHit::rows(lines.len()..lines.len() + 1, choice);
        hits.push(hit.with_active(selected));
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
    ApprovalFrame { lines, hits }
}

pub(super) fn approval_detail_line_count(prompt: &ApprovalPrompt, width: usize) -> usize {
    wrapped_detail_lines(prompt, width.max(1)).len()
}

pub(super) fn approval_detail_page_lines(viewport_height: usize) -> usize {
    viewport_height
        .saturating_sub(APPROVAL_DETAIL_CHROME_ROWS)
        .max(MIN_DETAIL_PAGE_LINES)
}

fn wrapped_detail_lines(prompt: &ApprovalPrompt, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for text in &prompt.details.primary {
        push_wrapped_text(&mut lines, text, width, Theme::text(), LineFill::Natural);
    }
    for text in &prompt.details.meta {
        push_wrapped_text(&mut lines, text, width, Theme::dim(), LineFill::Natural);
    }
    let reason = prompt.reason.trim();
    if !reason.is_empty() {
        push_wrapped_text(
            &mut lines,
            &format!("reason {}", sanitize_controls(reason)),
            width,
            Theme::dim(),
            LineFill::Natural,
        );
    }
    lines
}

pub(super) fn approval_title(request: &CapabilityRequest) -> String {
    let actor = match request.source() {
        CapabilitySource::BuiltInTool { name } => Actor::BuiltInTool(name),
        CapabilitySource::HostProvidedTool { name } => Actor::HostTool(name),
        CapabilitySource::PromptConstruction => Actor::Rho,
        _ => Actor::Rho,
    };
    title(actor, Some(request.kind()))
}

/// Who asks for a capability, as an approval title names it.
pub(super) enum Actor<'a> {
    BuiltInTool(&'a str),
    HostTool(&'a str),
    Rho,
}

/// "`actor` wants to `kind`'s verb"; `None` is a capability this rho
/// cannot name.
pub(super) fn title(actor: Actor<'_>, kind: Option<CapabilityKind>) -> String {
    // Host-provided tools are a different trust boundary from SDK built-ins.
    // Put the provenance marker first so narrow-title truncation keeps it.
    let actor = match actor {
        Actor::BuiltInTool(name) => sanitize_controls(name),
        Actor::HostTool(name) => format!("host {}", sanitize_controls(name)),
        Actor::Rho => "rho".into(),
    };
    let verb = match kind {
        Some(CapabilityKind::Write) => "write",
        Some(CapabilityKind::Process) => "run a command",
        Some(CapabilityKind::Read) => "read",
        Some(CapabilityKind::Network) => "access the network",
        Some(CapabilityKind::Skill) => "load a skill",
        Some(CapabilityKind::InstructionDiscovery) => "discover instructions",
        Some(_) | None => "use a capability",
    };
    format!("{actor} wants to {verb}")
}

/// Flattened detail strings for security and layout tests.
#[cfg(test)]
pub(super) fn approval_details(request: &CapabilityRequest) -> Vec<String> {
    let details = approval_detail_sections(request);
    let mut flat = details.primary;
    flat.extend(details.meta);
    flat
}

fn approval_detail_sections(request: &CapabilityRequest) -> ApprovalDetails {
    match request.operation() {
        CapabilityOperation::ReadPath { path, scope }
        | CapabilityOperation::WritePath { path, scope }
        | CapabilityOperation::DiscoverInstructions { path, scope } => ApprovalDetails {
            primary: vec![sanitize_controls(&path.to_string_lossy())],
            meta: vec![format_path_scope(scope)],
        },
        CapabilityOperation::ExecuteProcess(execution) => process_details(execution),
        CapabilityOperation::NetworkAccess(target) => ApprovalDetails {
            primary: vec![sanitize_controls(
                target.url().unwrap_or("tool-managed network access"),
            )],
            meta: Vec::new(),
        },
        CapabilityOperation::LoadSkill { name, path } => {
            let mut meta = Vec::new();
            if let Some(path) = path {
                meta.push(format!(
                    "path {}",
                    sanitize_controls(&path.to_string_lossy())
                ));
            }
            ApprovalDetails {
                primary: vec![sanitize_controls(name)],
                meta,
            }
        }
        _ => ApprovalDetails::default(),
    }
}

fn process_details(execution: &ProcessExecution) -> ApprovalDetails {
    let invocation = execution.invocation();
    let primary = if let Some(command) = invocation.shell_command() {
        sanitize_controls(command)
    } else {
        format_direct_invocation(invocation.executable_path(), invocation.arguments())
    };
    ApprovalDetails {
        primary: vec![primary],
        meta: vec![format_process_meta(execution)],
    }
}

fn format_process_meta(execution: &ProcessExecution) -> String {
    let invocation = execution.invocation();
    let mut parts = vec![format!(
        "cwd {}",
        sanitize_controls(&execution.working_directory().to_string_lossy())
    )];

    let lookup = format_executable_lookup(invocation.executable_selection());
    if invocation.shell_command().is_some() {
        parts.push(format!(
            "via {} ({lookup})",
            format_direct_invocation(invocation.executable_path(), invocation.arguments())
        ));
    } else {
        parts.push(lookup.into());
    }

    parts.push(format_environment_summary(execution.environment()));

    let limits = execution.output_limits();
    parts.push(format!("{} B out", limits.max_output_bytes()));
    parts.push(format_timeout(limits.timeout()));

    parts.join(" · ")
}

fn format_executable_lookup(selection: ExecutableSelection) -> &'static str {
    match selection {
        ExecutableSelection::ExactPath => "exact path",
        ExecutableSelection::SearchPath => "PATH",
        _ => "unspecified",
    }
}

pub(super) fn format_path_scope(scope: &PathScope) -> String {
    match scope {
        PathScope::PrimaryWorkspace => "scope primary workspace".into(),
        PathScope::GrantedRoot { root } => format!(
            "scope granted root {}",
            sanitize_controls(&root.to_string_lossy())
        ),
        PathScope::UnrestrictedFilesystem => "scope unrestricted filesystem".into(),
        _ => "scope unspecified".into(),
    }
}

fn format_environment_summary(environment: &ProcessEnvironment) -> String {
    match environment {
        ProcessEnvironment::Empty => "env empty".into(),
        ProcessEnvironment::InheritAll => "env inherit all".into(),
        ProcessEnvironment::InheritExcept { variable_names } => {
            // Scrub lists are not decision content; only the count matters.
            format!("env inherit ({} stripped)", variable_names.len())
        }
        // Values are secrets; only the count is shown.
        ProcessEnvironment::Explicit { variables } => {
            format!("env explicit ({} variables)", variables.len())
        }
        ProcessEnvironment::InheritListed { variable_names } => {
            // Allowlists are the authorization surface; show sanitized names.
            format!(
                "env {}",
                format_json_strings(variable_names.iter().map(String::as_str))
            )
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

pub(super) fn sanitize_controls(text: &str) -> String {
    escape_controls(text, LineBreaks::Escape)
}
