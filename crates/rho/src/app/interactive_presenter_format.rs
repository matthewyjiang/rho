//! Builds structured [`ToolCard`] values for interactive tool presentation.

use rho_sdk::tool::{OperationKind, ToolMetadata, ToolProgress};
use rho_tools::{
    tool::compact_display_path,
    tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus},
};

#[path = "interactive_presenter_results.rs"]
mod results;
use results::{
    count_nonempty_lines, fetch_content_card, file_diff_card, generic_card,
    get_search_content_card, process_result_card, push_error_output, search_result_card,
    shell_card, shell_result_card, split_body_lines, web_search_card,
};

use super::{agent_format, ToolKind, ToolPresentation, ToolView};

pub(super) fn presentation(view: &ToolView, mut card: ToolCard) -> ToolPresentation {
    card.push_notice_facts(view.metadata.presentation_notices());
    // Family is assigned only here so draft builders stay family-agnostic.
    card.family = family_for_kind(view.kind, Some(&view.metadata));
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

/// Draft card with a placeholder family; [`presentation`] sets the real one.
pub(super) fn draft_card(status: ToolStatus, header: ToolHeader) -> ToolCard {
    ToolCard::new(status, ToolFamily::Default, header)
}

pub(super) fn start_card(view: &ToolView, cwd: &std::path::Path) -> ToolCard {
    preview_card(
        view.kind,
        &view.name,
        Some(&view.arguments),
        cwd,
        ToolStatus::Running,
    )
}

/// Live tool-call argument preview while the provider is still streaming JSON.
pub(super) fn streaming_preview_card(
    kind: ToolKind,
    name: &str,
    raw_arguments: &str,
    cwd: &std::path::Path,
) -> ToolCard {
    match kind {
        ToolKind::Agent => agent_format::agent_streaming_preview_card(raw_arguments),
        _ => {
            let arguments = parse_incomplete_json(raw_arguments);
            preview_card(kind, name, arguments.as_ref(), cwd, ToolStatus::Running)
        }
    }
}

pub(super) fn preview_card(
    kind: ToolKind,
    name: &str,
    arguments: Option<&serde_json::Value>,
    cwd: &std::path::Path,
    status: ToolStatus,
) -> ToolCard {
    let Some(arguments) = arguments else {
        let header = match kind {
            ToolKind::Bash => ToolHeader::shell("$", None),
            ToolKind::PowerShell => ToolHeader::shell("PS", None),
            _ => ToolHeader::call(name, None),
        };
        return draft_card(status, header);
    };
    match kind {
        ToolKind::Agent => agent_format::agent_start_card(arguments),
        ToolKind::Agents => agent_format::agents_start_card(arguments),
        ToolKind::Bash => shell_card("$", arguments, status, None),
        ToolKind::PowerShell => shell_card("PS", arguments, status, None),
        ToolKind::Process => {
            let action = string_arg(arguments, "action");
            let primary = action.clone().or_else(|| string_arg(arguments, "command"));
            let mut card = draft_card(
                status,
                ToolHeader::call(
                    "process",
                    action.as_deref().map(str::to_string).or(primary.clone()),
                ),
            );
            if action.as_deref() == Some("start") {
                if let Some(command) = string_arg(arguments, "command") {
                    card.push_fact(ToolFact::Text { text: command });
                }
            }
            card
        }
        ToolKind::ListDir => draft_card(
            status,
            ToolHeader::call(
                "list_dir",
                Some(display_path(arguments, cwd)).filter(|p| !p.is_empty()),
            ),
        ),
        ToolKind::Grep => draft_card(
            status,
            ToolHeader::call("grep", Some(grep_primary(arguments, cwd))),
        ),
        ToolKind::Glob => draft_card(
            status,
            ToolHeader::call(
                "glob",
                string_arg(arguments, "pattern").filter(|pattern| !pattern.is_empty()),
            ),
        ),
        ToolKind::ReadFile => draft_card(
            status,
            ToolHeader::call(
                "read_file",
                Some(read_path(arguments, cwd)).filter(|p| !p.is_empty()),
            ),
        ),
        ToolKind::WriteFile => draft_card(
            status,
            ToolHeader::call(
                "write_file",
                Some(display_path(arguments, cwd)).filter(|p| !p.is_empty()),
            ),
        ),
        ToolKind::EditFile => edit_start_card(arguments, cwd, status),
        ToolKind::Skill => draft_card(
            status,
            ToolHeader::call(
                "skill",
                string_arg(arguments, "name").filter(|name| !name.is_empty()),
            ),
        ),
        ToolKind::Questionnaire => match crate::questionnaire::parse_request(arguments.clone()) {
            Ok(request) => {
                let primary = request.title.clone().or_else(|| Some(name.to_string()));
                let mut card = draft_card(status, ToolHeader::call("questionnaire", primary));
                for (index, question) in request.questions.iter().enumerate() {
                    card.push_fact(ToolFact::Text {
                        text: format!("{}. {}", index + 1, question.question),
                    });
                }
                card
            }
            Err(_) => draft_card(status, ToolHeader::call(name, None)),
        },
        ToolKind::Other => {
            if name == "rho" {
                return draft_card(
                    status,
                    ToolHeader::call(
                        "rho",
                        string_arg(arguments, "action").filter(|action| !action.is_empty()),
                    ),
                );
            }
            draft_card(status, ToolHeader::call(name, None))
        }
        ToolKind::WebSearch => {
            let primary = search_terms(arguments).or_else(|| Some(name.to_string()));
            draft_card(status, ToolHeader::call("web_search", primary))
        }
        ToolKind::FetchContent => {
            let primary = first_url(arguments).or_else(|| Some(name.to_string()));
            draft_card(status, ToolHeader::call("fetch_content", primary))
        }
        ToolKind::GetSearchContent => draft_card(
            status,
            ToolHeader::call("get_search_content", Some(get_search_primary(arguments))),
        ),
    }
}

fn edit_start_card(
    arguments: &serde_json::Value,
    cwd: &std::path::Path,
    status: ToolStatus,
) -> ToolCard {
    let paths = edit_paths(arguments, cwd);
    let primary = match paths.as_slice() {
        [] => None,
        [path] => Some(path.clone()),
        paths => Some(format!("{} files", paths.len())),
    };
    draft_card(status, ToolHeader::call("edit_file", primary))
}

pub(super) fn finished_card(
    view: &ToolView,
    content: &str,
    ok: bool,
    cwd: &std::path::Path,
) -> ToolCard {
    let status = ToolStatus::from_finished(ok);
    match view.kind {
        ToolKind::Agent => agent_format::agent_finished_card(view, content, ok),
        ToolKind::Agents => agent_format::agents_finished_card(view, content, ok),
        ToolKind::Bash => shell_result_card("$", &view.arguments, content, status),
        ToolKind::PowerShell => shell_result_card("PS", &view.arguments, content, status),
        ToolKind::Process => process_result_card(content, status),
        ToolKind::ListDir => {
            let mut card = draft_card(
                status,
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
            let mut card = draft_card(
                status,
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
        ToolKind::WriteFile | ToolKind::EditFile => file_diff_card(view, content, ok, cwd),
        ToolKind::Skill => preview_card(view.kind, &view.name, Some(&view.arguments), cwd, status),
        ToolKind::WebSearch => web_search_card(&view.arguments, content, status),
        ToolKind::FetchContent => fetch_content_card(&view.arguments, content, status),
        ToolKind::GetSearchContent => get_search_content_card(content, status),
        ToolKind::Questionnaire => {
            preview_card(view.kind, &view.name, Some(&view.arguments), cwd, status)
        }
        ToolKind::Other => generic_card(view, content, status),
    }
}

pub(super) fn progress_card(
    view: Option<(&ToolView, &std::path::Path)>,
    progress: &ToolProgress,
) -> ToolCard {
    if let Some((view, _)) = view {
        if view.kind == ToolKind::Agent {
            return agent_format::agent_progress_card(view, progress.text());
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
                progress.text(),
                ToolStatus::Running,
            );
        }
    }
    let mut card = view.map_or_else(
        || draft_card(ToolStatus::Running, ToolHeader::call("tool", None)),
        |(view, cwd)| start_card(view, cwd),
    );
    if !progress.text().trim().is_empty() {
        card.body = ToolBody::Lines(split_body_lines(progress.text()));
    }
    if let (Some(completed), total) = (progress.completed_units(), progress.total_units()) {
        card.push_fact(ToolFact::Progress { completed, total });
    }
    card
}

pub(super) fn interrupted_card(
    view: &ToolView,
    partial_arguments: &str,
    cwd: &std::path::Path,
) -> ToolCard {
    match view.kind {
        ToolKind::Agent => agent_format::agent_interrupted_card(&view.arguments),
        ToolKind::Agents => agent_format::agents_interrupted_card(&view.arguments),
        _ => {
            let mut card = preview_card(
                view.kind,
                &view.name,
                Some(&view.arguments),
                cwd,
                ToolStatus::Interrupted,
            );
            if !partial_arguments.is_empty() && card.body.is_empty() && card.facts.is_empty() {
                card.body = ToolBody::Lines(vec![partial_arguments.to_string()]);
            }
            card
        }
    }
}

fn family_for_kind(kind: ToolKind, metadata: Option<&ToolMetadata>) -> ToolFamily {
    match kind {
        ToolKind::Agent | ToolKind::Agents => ToolFamily::Agent,
        ToolKind::Bash
        | ToolKind::PowerShell
        | ToolKind::ListDir
        | ToolKind::Grep
        | ToolKind::Glob
        | ToolKind::ReadFile => ToolFamily::FileCommand,
        ToolKind::WriteFile | ToolKind::EditFile => ToolFamily::FileDiff,
        ToolKind::Skill => ToolFamily::Skill,
        ToolKind::WebSearch | ToolKind::FetchContent | ToolKind::GetSearchContent => {
            ToolFamily::Web
        }
        ToolKind::Questionnaire => ToolFamily::Form,
        ToolKind::Process | ToolKind::Other => metadata
            .map(family_from_metadata)
            .unwrap_or(ToolFamily::Default),
    }
}

fn family_from_metadata(metadata: &ToolMetadata) -> ToolFamily {
    match metadata.operation_kind() {
        Some(OperationKind::Read | OperationKind::Execute) => ToolFamily::FileCommand,
        Some(OperationKind::Write) => ToolFamily::FileDiff,
        Some(OperationKind::Network) => ToolFamily::Web,
        Some(OperationKind::Other(kind)) if kind == "questionnaire" => ToolFamily::Form,
        Some(OperationKind::Other(_)) | None | Some(_) => ToolFamily::Default,
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

pub(super) fn edit_paths(arguments: &serde_json::Value, cwd: &std::path::Path) -> Vec<String> {
    arguments
        .get("edits")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|edit| edit.get("path").and_then(|path| path.as_str()))
        .map(|path| compact_display_path(cwd, path))
        .collect()
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
