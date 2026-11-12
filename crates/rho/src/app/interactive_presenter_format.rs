//! Builds structured [`ToolCard`] values for interactive tool presentation.

use rho_protocol::display::{ToolBody, ToolCard, ToolCardStatus, ToolFact, ToolFamily, ToolHeader};
use rho_sdk::tool::{OperationKind, ToolMetadata, ToolProgress};
use rho_tools::{resolve_path, tool::compact_display_path};

#[path = "interactive_presenter_results.rs"]
mod results;
use results::{
    count_nonempty_lines, diff_card, fetch_content_card, file_diff_card, generic_card,
    get_search_content_card, mcp_preview_card, mcp_result_card, process_result_card,
    search_result_card, shell_card, shell_result_card, web_search_card, EmptyDiffState,
};
pub(in crate::app::interactive_presenter) use results::{push_error_output, split_body_lines};

#[path = "interactive_presenter_apply_patch.rs"]
mod apply_patch_format;
use apply_patch_format::apply_patch_card;

#[path = "interactive_presenter_str_replace.rs"]
mod str_replace_format;
use str_replace_format::str_replace_card;

#[path = "interactive_presenter_plan_exit.rs"]
mod plan_exit_format;

use super::{
    agent_format, codemode_format, sessions_format, todo_format, tool_search_format, ToolKind,
    ToolPresentation, ToolView,
};
use crate::tools::mcp::exported_name::McpToolNames;

pub(super) fn presentation(view: &ToolView, mut card: ToolCard) -> ToolPresentation {
    card.push_notice_facts(view.metadata.presentation_notices());
    // Metadata can refine Process/Other family after start; keep builders honest.
    card.family = family_for_kind(&view.kind, Some(&view.metadata));
    ToolPresentation {
        card,
        image_asset: view
            .metadata
            .assets()
            .iter()
            .find(|asset| asset.media_type().starts_with("image/"))
            .cloned(),
    }
}

/// Build a card with a real family so intermediate state stays renderable.
pub(super) fn draft_card(
    status: ToolCardStatus,
    family: ToolFamily,
    header: ToolHeader,
) -> ToolCard {
    ToolCard::new(status, family, header)
}

fn kind_card(status: ToolCardStatus, kind: &ToolKind, header: ToolHeader) -> ToolCard {
    draft_card(status, family_for_kind(kind, None), header)
}

pub(super) fn start_card(view: &ToolView, cwd: &std::path::Path) -> ToolCard {
    let card = preview_card(
        &view.kind,
        &view.name,
        Some(&view.arguments),
        cwd,
        ToolCardStatus::Running,
    );
    with_summary_primary(&view.kind, &view.metadata, card)
}

/// [`start_card`] for a call that already ran, as a saved transcript shows
/// it. The file a `str_replace` changed no longer shows the change the call
/// was about to make, so its card projects the call's arguments instead,
/// the same each time it is drawn.
pub(super) fn saved_start_card(view: &ToolView, cwd: &std::path::Path) -> ToolCard {
    match &view.kind {
        ToolKind::Edit(rho_tools::EditFormat::StrReplace) => str_replace_card(
            &view.arguments,
            cwd,
            ToolCardStatus::Running,
            /*live_file*/ None,
        ),
        _ => start_card(view, cwd),
    }
}

/// Generic tools describe a call through metadata; a `command_summary` fills
/// the bare `name` header so a running card says what it is doing.
fn with_summary_primary(kind: &ToolKind, metadata: &ToolMetadata, mut card: ToolCard) -> ToolCard {
    if *kind != ToolKind::Other {
        return card;
    }
    if let (ToolHeader::Call { primary, .. }, Some(summary)) =
        (&mut card.header, metadata.command_summary_text())
    {
        if primary.is_none() && !summary.is_empty() {
            *primary = Some(summary.to_string());
        }
    }
    card
}

/// The interactive card's header for a call, for hosts that title each tool
/// call with one string, such as ACP clients. Built from the live-preview
/// card, which reads no files, so it is cheap enough to run per call.
pub(crate) fn call_header(
    name: &str,
    arguments: Option<&serde_json::Value>,
    metadata: &ToolMetadata,
    cwd: &std::path::Path,
    mcp_tools: &McpToolNames,
) -> ToolHeader {
    let kind = ToolKind::from_name(name, mcp_tools);
    let card = streaming_preview_card(
        &kind,
        name,
        arguments,
        cwd,
        // A started or replayed call's arguments are complete.
        rho_tools::apply_patch::ProposedDiffTrailingLine::Include,
    );
    with_summary_primary(&kind, metadata, card).header
}

/// The finished card of hosted tool `name`, which the provider ran itself,
/// such as `web_search`, with `detail` from its activity: a verb-only
/// header, then the detail when present, then "finished". The header keeps
/// operator-heavy queries out of `verb(primary)` parens and gives every
/// hosted lookup the same chrome. Hosted lookups return no result body to
/// the client, so the "finished" fact keeps the card from being a bare
/// header.
pub(crate) fn hosted_activity_card(name: &str, detail: &str) -> ToolCard {
    let family = match name {
        // Hosted X Search is a network lookup too.
        "web_search" | "x_search" => ToolFamily::Web,
        _ => ToolFamily::Default,
    };
    let mut card = ToolCard::new(ToolCardStatus::Ok, family, ToolHeader::call(name, None));
    let detail = detail.trim();
    if !detail.is_empty() {
        card.push_fact(ToolFact::Text {
            text: detail.to_owned(),
        });
    }
    card.push_fact(ToolFact::Meta {
        text: "finished".into(),
    });
    card
}

/// Live tool-call argument preview while the provider is still streaming
/// JSON. Builds from the arguments alone and must not read files:
/// [`call_header`] runs it for every ACP tool call. `trailing_line` says
/// whether a patch's last unterminated line is final or still streaming.
pub(super) fn streaming_preview_card(
    kind: &ToolKind,
    name: &str,
    arguments: Option<&serde_json::Value>,
    cwd: &std::path::Path,
    trailing_line: rho_tools::apply_patch::ProposedDiffTrailingLine,
) -> ToolCard {
    match kind {
        ToolKind::Agent => agent_format::agent_start_card(
            arguments.unwrap_or(&serde_json::Value::Object(Default::default())),
        ),
        ToolKind::Edit(format) => arguments.map_or_else(
            || kind_card(ToolCardStatus::Running, kind, ToolHeader::call(name, None)),
            |arguments| match format {
                rho_tools::EditFormat::ApplyPatch => {
                    apply_patch_card(arguments, cwd, ToolCardStatus::Running, trailing_line)
                }
                // Streaming skips the file read; rows get numbers once proposed.
                rho_tools::EditFormat::StrReplace => {
                    str_replace_card(
                        arguments,
                        cwd,
                        ToolCardStatus::Running,
                        /*live_file*/ None,
                    )
                }
                // Published enum stays non_exhaustive; unknown future formats
                // get a generic path/header card.
                _ => preview_card(kind, name, Some(arguments), cwd, ToolCardStatus::Running),
            },
        ),
        _ => preview_card(kind, name, arguments, cwd, ToolCardStatus::Running),
    }
}

pub(super) fn preview_card(
    kind: &ToolKind,
    name: &str,
    arguments: Option<&serde_json::Value>,
    cwd: &std::path::Path,
    status: ToolCardStatus,
) -> ToolCard {
    let Some(arguments) = arguments else {
        return match kind {
            ToolKind::Bash => kind_card(status, kind, ToolHeader::shell("$", None)),
            ToolKind::PowerShell => kind_card(status, kind, ToolHeader::shell("PS", None)),
            ToolKind::Mcp(identity) => mcp_preview_card(identity, None, status),
            _ => kind_card(status, kind, ToolHeader::call(name, None)),
        };
    };
    match kind {
        // Status-first like a nested agent: phase lives on the header line.
        ToolKind::Advisor => kind_card(
            status,
            kind,
            ToolHeader::status_first(
                "advisor",
                crate::agent::OneShotPhase::WaitingForProvider.label(),
            ),
        ),
        ToolKind::Agent => agent_format::agent_start_card(arguments),
        ToolKind::Agents => agent_format::agents_start_card(arguments),
        ToolKind::Bash => shell_card("$", arguments, status),
        ToolKind::PowerShell => shell_card("PS", arguments, status),
        ToolKind::Process => {
            let action = string_arg(arguments, "action");
            let primary = action.clone().or_else(|| string_arg(arguments, "command"));
            let mut card = kind_card(status, kind, ToolHeader::call("process", primary));
            if action.as_deref() == Some("start") {
                if let Some(command) = string_arg(arguments, "command") {
                    card.push_fact(ToolFact::Text { text: command });
                }
            }
            card
        }
        ToolKind::ListDir => kind_card(
            status,
            kind,
            ToolHeader::call(
                "list_dir",
                Some(display_path(arguments, cwd)).filter(|p| !p.is_empty()),
            ),
        ),
        ToolKind::Grep => kind_card(
            status,
            kind,
            ToolHeader::call("grep", Some(grep_primary(arguments, cwd))),
        ),
        ToolKind::Glob => kind_card(
            status,
            kind,
            ToolHeader::call(
                "glob",
                string_arg(arguments, "pattern").filter(|pattern| !pattern.is_empty()),
            ),
        ),
        ToolKind::ReadFile => kind_card(
            status,
            kind,
            ToolHeader::call(
                "read_file",
                Some(read_path(arguments, cwd)).filter(|p| !p.is_empty()),
            ),
        ),
        ToolKind::WriteFile => {
            let mut card = kind_card(
                status,
                kind,
                ToolHeader::call(
                    "write",
                    Some(display_path(arguments, cwd)).filter(|p| !p.is_empty()),
                ),
            );
            // Preview the payload, not a guessed diff against the current file.
            // Completion replaces this with the tool's actual diff.
            if let Some(content) = arguments.get("content").and_then(serde_json::Value::as_str) {
                card.body = ToolBody::Lines(split_body_lines(content));
            }
            card
        }
        ToolKind::Edit(format) => edit_preview_card(*format, arguments, cwd, status),
        ToolKind::Skill => kind_card(
            status,
            kind,
            ToolHeader::call(
                "skill",
                string_arg(arguments, "name").filter(|name| !name.is_empty()),
            ),
        ),
        ToolKind::Todo => todo_format::card(arguments, status),
        ToolKind::ExitPlanMode => plan_exit_format::card(arguments, status),
        ToolKind::Questionnaire => match crate::questionnaire::parse_request(arguments.clone()) {
            Ok(request) => {
                let primary = request.title.clone().or_else(|| Some(name.to_string()));
                let mut card = kind_card(status, kind, ToolHeader::call("questionnaire", primary));
                for (index, question) in request.questions.iter().enumerate() {
                    card.push_fact(ToolFact::Text {
                        text: format!("{}. {}", index + 1, question.question),
                    });
                }
                card
            }
            Err(_) => kind_card(status, kind, ToolHeader::call(name, None)),
        },
        ToolKind::Other => {
            if name == "rho" {
                return kind_card(
                    status,
                    kind,
                    ToolHeader::call(
                        "rho",
                        string_arg(arguments, "action").filter(|action| !action.is_empty()),
                    ),
                );
            }
            kind_card(status, kind, ToolHeader::call(name, None))
        }
        ToolKind::Sessions => sessions_format::preview_card(arguments, status),
        ToolKind::Workflow => crate::presentation::workflow::preview_card(arguments, cwd, status),
        ToolKind::ToolSearch => tool_search_format::preview_card(arguments, status),
        ToolKind::Codemode => {
            codemode_format::preview_card(arguments, status, /*primary*/ None)
        }
        ToolKind::WebSearch => {
            let primary = search_terms(arguments).or_else(|| Some(name.to_string()));
            kind_card(status, kind, ToolHeader::call("web_search", primary))
        }
        ToolKind::FetchContent => {
            let primary = first_url(arguments).or_else(|| Some(name.to_string()));
            kind_card(status, kind, ToolHeader::call("fetch_content", primary))
        }
        ToolKind::GetSearchContent => kind_card(
            status,
            kind,
            ToolHeader::call("get_search_content", Some(get_search_primary(arguments))),
        ),
        ToolKind::Mcp(identity) => mcp_preview_card(identity, Some(arguments), status),
    }
}

fn edit_preview_card(
    format: rho_tools::EditFormat,
    arguments: &serde_json::Value,
    cwd: &std::path::Path,
    status: ToolCardStatus,
) -> ToolCard {
    match format {
        rho_tools::EditFormat::ApplyPatch => apply_patch_card(
            arguments,
            cwd,
            status,
            rho_tools::apply_patch::ProposedDiffTrailingLine::Include,
        ),
        rho_tools::EditFormat::StrReplace => {
            let live_file =
                string_arg(arguments, "path").and_then(|path| read_planned_edit_file(cwd, &path));
            str_replace_card(arguments, cwd, status, live_file.as_deref())
        }
        // Published enum stays non_exhaustive; unknown future formats get a
        // path header.
        _ => kind_card(
            status,
            &ToolKind::Edit(format),
            ToolHeader::call(
                format.tool_name(),
                Some(display_path(arguments, cwd)).filter(|path| !path.is_empty()),
            ),
        ),
    }
}

/// Max bytes per file for interactive edit planning (sync path).
const MAX_EDIT_PLAN_FILE_BYTES: u64 = 1_048_576;

/// Resolve `path` inside `cwd`, reject escapes, and read only small enough files.
fn read_planned_edit_file(cwd: &std::path::Path, path: &str) -> Option<String> {
    let candidate = resolve_path(cwd, path);
    let cwd_canon = std::fs::canonicalize(cwd).ok()?;
    let path_canon = std::fs::canonicalize(&candidate).ok()?;
    if !path_canon.starts_with(&cwd_canon) {
        return None;
    }
    let meta = std::fs::metadata(&path_canon).ok()?;
    if !meta.is_file() || meta.len() > MAX_EDIT_PLAN_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path_canon).ok()
}

pub(super) fn finished_card(
    view: &ToolView,
    content: &str,
    ok: bool,
    cwd: &std::path::Path,
    data: Option<&serde_json::Value>,
) -> ToolCard {
    let status = ToolCardStatus::from_finished(ok);
    match &view.kind {
        ToolKind::Advisor => advisor_finished_card(content, ok),
        ToolKind::Agent => agent_format::agent_finished_card(view, content, ok),
        ToolKind::Agents => agent_format::agents_finished_card(view, content, ok),
        ToolKind::Bash => shell_result_card("$", &view.arguments, content, status),
        ToolKind::PowerShell => shell_result_card("PS", &view.arguments, content, status),
        ToolKind::Process => process_result_card(content, status),
        ToolKind::ListDir => {
            let mut card = kind_card(
                status,
                &view.kind,
                ToolHeader::call(
                    "list_dir",
                    Some(metadata_path(view, cwd)).filter(|path| !path.is_empty()),
                ),
            );
            if !ok && !content.trim().is_empty() {
                push_error_output(&mut card, content);
            } else if let Some(count) = count_nonempty_lines(content) {
                card.push_fact(ToolFact::Count {
                    label: if count == 1 {
                        "entry".into()
                    } else {
                        "entries".into()
                    },
                    value: count,
                    detail: None,
                });
            }
            card
        }
        ToolKind::Grep | ToolKind::Glob => search_result_card(view, content, ok, cwd),
        ToolKind::ReadFile => {
            let mut card = kind_card(
                status,
                &view.kind,
                ToolHeader::call(
                    "read_file",
                    Some(metadata_read_path(view, cwd)).filter(|path| !path.is_empty()),
                ),
            );
            if !ok && !content.trim().is_empty() {
                push_error_output(&mut card, content);
            } else if let Some(count) = count_nonempty_lines(content) {
                card.push_fact(ToolFact::Count {
                    label: if count == 1 {
                        "line".into()
                    } else {
                        "lines".into()
                    },
                    value: count,
                    detail: None,
                });
            }
            card
        }
        ToolKind::WriteFile | ToolKind::Edit(_) => file_diff_card(view, content, ok, cwd),
        // Output becomes the body; a failure surfaces its reason as an error fact.
        ToolKind::Skill | ToolKind::Sessions => {
            let mut card = preview_card(&view.kind, &view.name, Some(&view.arguments), cwd, status);
            if ok {
                card.body = ToolBody::Lines(split_body_lines(content));
            } else {
                push_error_output(&mut card, content);
            }
            card
        }
        ToolKind::WebSearch => web_search_card(&view.arguments, content, status),
        ToolKind::FetchContent => fetch_content_card(&view.arguments, content, status),
        ToolKind::GetSearchContent => get_search_content_card(content, status),
        ToolKind::Todo => todo_format::finished_card(&view.arguments, content, ok),
        ToolKind::ExitPlanMode => {
            let mut card = plan_exit_format::card(&view.arguments, status);
            if !ok {
                push_error_output(&mut card, content);
            }
            card
        }
        ToolKind::Questionnaire => {
            preview_card(&view.kind, &view.name, Some(&view.arguments), cwd, status)
        }
        ToolKind::Codemode => codemode_format::finished_card(&view.arguments, content, ok, data),
        ToolKind::ToolSearch => tool_search_format::finished_card(&view.arguments, content, ok),
        ToolKind::Workflow => {
            crate::presentation::workflow::finished_card(&view.arguments, content, ok, cwd)
        }
        ToolKind::Mcp(identity) => mcp_result_card(identity, view, content, status),
        ToolKind::Other => generic_card(view, content, status),
    }
}

pub(super) fn progress_card(
    view: Option<(&ToolView, &std::path::Path)>,
    progress: &ToolProgress,
) -> ToolCard {
    if let Some((view, _)) = view {
        if view.kind == ToolKind::Advisor {
            return advisor_progress_card(progress);
        }
        if view.kind == ToolKind::Agent {
            return agent_format::agent_progress_card(view, progress.message());
        }
        if view.kind == ToolKind::Codemode {
            return codemode_format::preview_card(
                &view.arguments,
                ToolCardStatus::Running,
                /*primary*/ None,
            );
        }
        if matches!(view.kind, ToolKind::Bash | ToolKind::PowerShell) {
            let prompt = if view.kind == ToolKind::Bash {
                "$"
            } else {
                "PS"
            };
            return shell_result_card(
                prompt,
                &view.arguments,
                progress.message(),
                ToolCardStatus::Running,
            );
        }
    }
    let mut card = view.map_or_else(
        || {
            kind_card(
                ToolCardStatus::Running,
                &ToolKind::Other,
                ToolHeader::call("tool", None),
            )
        },
        |(view, cwd)| start_card(view, cwd),
    );
    if !progress.message().trim().is_empty() {
        if card.body.is_diff() {
            // Keep an existing proposed/result diff visible; progress is metadata.
            card.push_fact(ToolFact::Meta {
                text: progress.message().to_string(),
            });
        } else {
            card.body = ToolBody::Lines(split_body_lines(progress.message()));
        }
    }
    if let (Some(completed), total) = (progress.completed_units(), progress.total_units()) {
        card.push_fact(ToolFact::Progress { completed, total });
    }
    card
}

fn advisor_card(status: ToolCardStatus, detail: impl Into<String>) -> ToolCard {
    kind_card(
        status,
        &ToolKind::Advisor,
        ToolHeader::status_first("advisor", detail),
    )
}

fn advisor_progress_card(progress: &ToolProgress) -> ToolCard {
    let phase = progress
        .metadata()
        .command_summary_text()
        .unwrap_or(crate::agent::OneShotPhase::WaitingForProvider.label());
    let mut card = advisor_card(ToolCardStatus::Running, phase);
    if !progress.message().trim().is_empty() {
        card.body = ToolBody::Lines(split_body_lines(progress.message()));
    }
    card
}

fn advisor_finished_card(content: &str, ok: bool) -> ToolCard {
    let status = ToolCardStatus::from_finished(ok);
    let mut card = advisor_card(status, if ok { "completed" } else { "failed" });
    if content.trim().is_empty() {
        return card;
    }
    if status == ToolCardStatus::Error {
        push_error_output(&mut card, content);
    } else {
        card.body = ToolBody::Lines(split_body_lines(content));
    }
    card
}

pub(super) fn family_for_kind(kind: &ToolKind, metadata: Option<&ToolMetadata>) -> ToolFamily {
    match kind {
        // The advisor is another model reviewing the run, like a subagent.
        ToolKind::Advisor | ToolKind::Agent | ToolKind::Agents => ToolFamily::Agent,
        ToolKind::Bash
        | ToolKind::PowerShell
        | ToolKind::ListDir
        | ToolKind::Grep
        | ToolKind::Glob
        | ToolKind::ReadFile => ToolFamily::FileCommand,
        ToolKind::WriteFile | ToolKind::Edit(_) => ToolFamily::FileDiff,
        ToolKind::Skill => ToolFamily::Skill,
        ToolKind::WebSearch | ToolKind::FetchContent | ToolKind::GetSearchContent => {
            ToolFamily::Web
        }
        ToolKind::Questionnaire | ToolKind::ExitPlanMode | ToolKind::Todo => ToolFamily::Form,
        ToolKind::Mcp(_)
        | ToolKind::Sessions
        | ToolKind::Codemode
        | ToolKind::ToolSearch
        | ToolKind::Workflow => ToolFamily::Default,
        ToolKind::Process | ToolKind::Other => metadata
            .map(family_from_metadata)
            .unwrap_or(ToolFamily::Default),
    }
}

fn family_from_metadata(metadata: &ToolMetadata) -> ToolFamily {
    match metadata.operation_kind() {
        Some(OperationKind::Read | OperationKind::Search | OperationKind::Execute) => {
            ToolFamily::FileCommand
        }
        Some(OperationKind::Write) => ToolFamily::FileDiff,
        Some(OperationKind::Network) => ToolFamily::Web,
        Some(OperationKind::Other(kind)) if kind == "questionnaire" => ToolFamily::Form,
        Some(OperationKind::Other(_)) | None => ToolFamily::Default,
        // OperationKind is non_exhaustive; unknown future variants stay default.
        Some(_) => ToolFamily::Default,
    }
}

pub(super) fn string_arg(arguments: &serde_json::Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

pub(super) fn display_path(arguments: &serde_json::Value, cwd: &std::path::Path) -> String {
    string_arg(arguments, "path")
        .map(|path| compact_display_path(cwd, &path))
        .unwrap_or_default()
}

fn grep_primary(arguments: &serde_json::Value, cwd: &std::path::Path) -> String {
    let pattern = string_arg(arguments, "pattern").unwrap_or_default();
    let path = display_path(arguments, cwd);
    if path.is_empty() {
        pattern
    } else if pattern.is_empty() {
        path
    } else {
        format!("{pattern}, {path}")
    }
}

pub(super) fn read_path(arguments: &serde_json::Value, cwd: &std::path::Path) -> String {
    let path = display_path(arguments, cwd);
    let offset = arguments.get("offset").and_then(|value| value.as_u64());
    let limit = arguments.get("limit").and_then(|value| value.as_u64());
    if offset.is_none() && limit.is_none() {
        return path;
    }
    let start = offset.unwrap_or(1);
    let end = limit.map_or_else(
        || "end".into(),
        |limit| start.saturating_add(limit).saturating_sub(1).to_string(),
    );
    format!("{path}:{start}-{end}")
}

pub(super) fn edit_paths(
    format: rho_tools::EditFormat,
    arguments: &serde_json::Value,
    cwd: &std::path::Path,
) -> Vec<String> {
    match format {
        rho_tools::EditFormat::StrReplace => {
            let path = display_path(arguments, cwd);
            (!path.is_empty()).then_some(path).into_iter().collect()
        }
        rho_tools::EditFormat::ApplyPatch => arguments
            .get("input")
            .and_then(|value| value.as_str())
            .map(rho_tools::apply_patch::patch_paths_lenient)
            .unwrap_or_default()
            .into_iter()
            .map(|path| compact_display_path(cwd, &path))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn metadata_paths(view: &ToolView, cwd: &std::path::Path) -> Vec<String> {
    view.metadata
        .affected_paths()
        .iter()
        .map(|path| compact_display_path(cwd, &path.to_string_lossy()))
        .collect()
}

pub(super) fn metadata_path(view: &ToolView, cwd: &std::path::Path) -> String {
    metadata_paths(view, cwd)
        .into_iter()
        .next()
        .unwrap_or_else(|| display_path(&view.arguments, cwd))
}

pub(super) fn metadata_read_path(view: &ToolView, cwd: &std::path::Path) -> String {
    metadata_paths(view, cwd)
        .into_iter()
        .next()
        .unwrap_or_else(|| read_path(&view.arguments, cwd))
}

pub(super) fn search_terms(arguments: &serde_json::Value) -> Option<String> {
    let terms = arguments
        .get("queries")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .or_else(|| {
            arguments
                .get("query")
                .and_then(|value| value.as_str())
                .map(|value| vec![value])
        })?;
    let mut rendered = terms
        .iter()
        .take(3)
        .map(|value| quoted(value, 48))
        .collect::<Vec<_>>();
    if terms.len() > rendered.len() {
        rendered.push(format!("{} more", terms.len() - rendered.len()));
    }
    Some(rendered.join(", "))
}

pub(super) fn first_url(arguments: &serde_json::Value) -> Option<String> {
    arguments
        .get("urls")
        .and_then(|value| value.as_array())
        .and_then(|urls| urls.first())
        .and_then(|value| value.as_str())
        .map(|url| truncate(url, 80))
        .or_else(|| string_arg(arguments, "url").map(|url| truncate(&url, 80)))
}

fn get_search_primary(arguments: &serde_json::Value) -> String {
    string_arg(arguments, "query")
        .map(|query| quoted(&query, 48))
        .or_else(|| string_arg(arguments, "url").map(|url| truncate(&url, 48)))
        .or_else(|| {
            arguments
                .get("responseId")
                .and_then(|value| value.as_str())
                .map(|id| truncate(id, 24))
        })
        .unwrap_or_else(|| "...".into())
}

pub(super) fn quoted(value: &str, max: usize) -> String {
    format!("\"{}\"", truncate(value, max))
}

pub(super) fn truncate(value: &str, max: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max {
        return value;
    }
    let mut value = value
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>();
    value.push('…');
    value
}

pub(super) fn parse_incomplete_json(input: &str) -> Option<serde_json::Value> {
    serde_json::from_str(input)
        .ok()
        .or_else(|| complete_partial_json(input))
}

pub(super) fn complete_partial_json(input: &str) -> Option<serde_json::Value> {
    let mut suffix = String::new();
    let mut containers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for character in input.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else {
                match character {
                    '\\' => escaped = true,
                    '"' => in_string = false,
                    _ => {}
                }
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => containers.push('}'),
            '[' => containers.push(']'),
            '}' | ']' => {
                containers.pop();
            }
            _ => {}
        }
    }
    if in_string {
        if escaped {
            suffix.push('\\');
        }
        suffix.push('"');
    }
    suffix.extend(containers.into_iter().rev());
    serde_json::from_str(&format!("{input}{suffix}")).ok()
}
