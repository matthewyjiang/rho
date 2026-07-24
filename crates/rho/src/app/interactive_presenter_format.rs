use rho_sdk::tool::{OperationKind, ToolMetadata, ToolProgress};

use rho_tools::tool::{compact_display_path, ToolDisplayStyle};

use super::{agent_format, ToolKind, ToolPresentation, ToolView};

pub(super) fn presentation(view: &ToolView, mut display_lines: Vec<String>) -> ToolPresentation {
    display_lines.extend(view.metadata.presentation_notices().iter().cloned());
    ToolPresentation {
        command: command(view),
        display_style: view.kind.display_style(&view.metadata),
        display_lines,
        image_asset: view
            .metadata
            .assets()
            .iter()
            .find(|asset| asset.media_type().starts_with("image/"))
            .cloned(),
    }
}

pub(super) fn command(view: &ToolView) -> Option<String> {
    view.metadata
        .command_summary_text()
        .map(str::to_string)
        .or_else(|| match view.kind {
            ToolKind::Bash | ToolKind::PowerShell => string_arg(&view.arguments, "command"),
            ToolKind::Process
                if view.arguments.get("action").and_then(|v| v.as_str()) == Some("start") =>
            {
                string_arg(&view.arguments, "command")
            }
            _ => None,
        })
}

pub(super) fn start_lines(view: &ToolView, cwd: &std::path::Path) -> Vec<String> {
    preview_lines(view.kind, &view.name, Some(&view.arguments), cwd)
}

pub(super) fn preview_lines(
    kind: ToolKind,
    name: &str,
    arguments: Option<&serde_json::Value>,
    cwd: &std::path::Path,
) -> Vec<String> {
    let Some(arguments) = arguments else {
        return vec![match kind {
            ToolKind::Bash => "$".into(),
            ToolKind::PowerShell => "PS".into(),
            _ => name.to_string(),
        }];
    };
    match kind {
        ToolKind::Agent => agent_format::agent_start_lines_for(arguments),
        ToolKind::Agents => agent_format::agents_start_lines_for(arguments),
        ToolKind::Bash => vec![command_line("$", arguments)],
        ToolKind::PowerShell => vec![command_line("PS", arguments)],
        ToolKind::Process => {
            let mut lines = vec!["process".into()];
            if arguments.get("action").and_then(|value| value.as_str()) == Some("start") {
                if let Some(command) = string_arg(arguments, "command") {
                    lines.push(command);
                }
            }
            lines
        }
        ToolKind::ListDir => vec![format!("list_dir {}", display_path(arguments, cwd))],
        ToolKind::ReadFile => vec![format!("read_file {}", read_path(arguments, cwd))],
        ToolKind::WriteFile => vec![format!("write_file {}", display_path(arguments, cwd))],
        ToolKind::EditFile => vec![format!(
            "edit_file {}",
            edit_paths(arguments, cwd).join(", ")
        )],
        ToolKind::Skill => vec![string_arg(arguments, "name")
            .map_or_else(|| "skill".into(), |name| format!("skill {name}"))],
        ToolKind::Questionnaire => crate::questionnaire::parse_request(arguments.clone())
            .map(|request| crate::questionnaire::start_display_lines(&request))
            .unwrap_or_else(|_| vec![name.to_string()]),
        ToolKind::Other => {
            if name == "rho" {
                return vec![string_arg(arguments, "action")
                    .map_or_else(|| "rho".into(), |action| format!("rho {action}"))];
            }
            vec![name.to_string()]
        }
        ToolKind::WebSearch | ToolKind::FetchContent | ToolKind::GetSearchContent => {
            vec![name.to_string()]
        }
    }
}

pub(super) fn finished_lines(
    view: &ToolView,
    content: &str,
    ok: bool,
    cwd: &std::path::Path,
) -> Vec<String> {
    match view.kind {
        ToolKind::Agent => agent_format::agent_finished_lines(view, content, ok),
        ToolKind::Agents => agent_format::agents_finished_lines(view, content, ok),
        ToolKind::Bash => command_result_lines("$", &view.arguments, content),
        ToolKind::PowerShell => command_result_lines("PS", &view.arguments, content),
        ToolKind::Process => process_result_lines(content),
        ToolKind::ListDir => vec![format!("list_dir {}", metadata_path(view, cwd))],
        ToolKind::ReadFile => vec![format!("read_file {}", metadata_read_path(view, cwd))],
        ToolKind::WriteFile | ToolKind::EditFile => file_diff_lines(view, content, ok, cwd),
        ToolKind::Skill => preview_lines(view.kind, &view.name, Some(&view.arguments), cwd),
        ToolKind::WebSearch => vec![web_search_line(&view.arguments, content)],
        ToolKind::FetchContent => vec![fetch_content_line(content)],
        ToolKind::GetSearchContent => vec![get_search_content_line(content)],
        ToolKind::Questionnaire => preview_lines(view.kind, &view.name, Some(&view.arguments), cwd),
        ToolKind::Other => generic_lines(view, content),
    }
}

pub(super) fn progress_lines(
    view: Option<(&ToolView, &std::path::Path)>,
    progress: &ToolProgress,
) -> Vec<String> {
    if let Some((view, _)) = view {
        if view.kind == ToolKind::Agent {
            return agent_format::agent_progress_lines(view, progress.text());
        }
        if matches!(view.kind, ToolKind::Bash | ToolKind::PowerShell) {
            let prompt = if view.kind == ToolKind::Bash {
                "$"
            } else {
                "PS"
            };
            return command_result_lines(prompt, &view.arguments, progress.text());
        }
    }
    let mut lines = view.map_or_else(|| vec!["tool".into()], |(view, cwd)| start_lines(view, cwd));
    if !progress.text().trim().is_empty() {
        lines.push(progress.text().to_string());
    }
    if let (Some(completed), Some(total)) = (progress.completed_units(), progress.total_units()) {
        lines.push(format!("progress: {completed}/{total}"));
    }
    lines
}

pub(super) fn file_diff_lines(
    view: &ToolView,
    content: &str,
    ok: bool,
    cwd: &std::path::Path,
) -> Vec<String> {
    let paths = metadata_paths(view, cwd);
    let label = if paths.is_empty() {
        if view.kind == ToolKind::EditFile {
            edit_paths(&view.arguments, cwd).join(", ")
        } else {
            display_path(&view.arguments, cwd)
        }
    } else {
        paths.join(", ")
    };
    let mut lines = vec![format!("{} {label}", view.name)];
    if ok {
        let diff = view.metadata.unified_diff().unwrap_or(content);
        let compact = compact_diff(diff, paths.len() > 1);
        if let Some(compact) = compact {
            lines.push(compact);
        }
    } else if !content.trim().is_empty() {
        lines.push(content.to_string());
    }
    lines
}

pub(super) fn generic_lines(view: &ToolView, content: &str) -> Vec<String> {
    let mut lines = vec![view.name.clone()];
    if let Some(command) = view.metadata.command_summary_text() {
        lines.push(command.to_string());
    }
    lines.extend(
        view.metadata
            .affected_paths()
            .iter()
            .map(|path| path.display().to_string()),
    );
    lines.extend(view.metadata.urls().iter().cloned());
    if let Some(diff) = view.metadata.unified_diff() {
        lines.push(diff.to_string());
    }
    if lines.len() == 1 && view.arguments != serde_json::Value::Object(Default::default()) {
        lines.push(view.arguments.to_string());
    }
    if !content.trim().is_empty() {
        lines.push(content.to_string());
    }
    lines
}

pub(super) fn style_from_metadata(metadata: &ToolMetadata) -> ToolDisplayStyle {
    match metadata.operation_kind() {
        Some(OperationKind::Read | OperationKind::Execute) => ToolDisplayStyle::file_or_command(),
        Some(OperationKind::Write) => ToolDisplayStyle::file_diff(),
        Some(OperationKind::Network) => ToolDisplayStyle::web(),
        Some(OperationKind::Other(kind)) if kind == "questionnaire" => {
            ToolDisplayStyle::questionnaire()
        }
        Some(OperationKind::Other(_)) | None | Some(_) => ToolDisplayStyle::default_tool(),
    }
}

pub(super) fn string_arg(arguments: &serde_json::Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

pub(super) fn command_line(shell: &str, arguments: &serde_json::Value) -> String {
    string_arg(arguments, "command")
        .filter(|command| !command.trim().is_empty())
        .map_or_else(|| shell.to_string(), |command| format!("{shell} {command}"))
}

pub(super) fn command_result_lines(
    shell: &str,
    arguments: &serde_json::Value,
    content: &str,
) -> Vec<String> {
    let mut lines = vec![command_line(shell, arguments)];
    lines.push(
        arguments
            .get("timeout_seconds")
            .and_then(|value| value.as_u64())
            .map_or_else(
                || "timeout: none".into(),
                |seconds| format!("timeout: {seconds}s"),
            ),
    );
    if let Some((notice, stdout)) = shell_output(content) {
        if let Some(notice) = notice {
            lines.push(notice.to_string());
        }
        if !stdout.trim().is_empty() {
            lines.push(String::new());
            lines.push(stdout.trim_end().to_string());
        }
    } else if !content.trim().is_empty() {
        lines.push(String::new());
        lines.push(content.to_string());
    }
    lines
}

fn shell_output(content: &str) -> Option<(Option<&str>, &str)> {
    let (notice, output) = if let Some(stdout) = content.strip_prefix("stdout:\n") {
        (None, stdout)
    } else {
        let (notice, stdout) = content.split_once("\n\nstdout:\n")?;
        (Some(notice), stdout)
    };
    let stdout = output
        .rsplit_once("\n\nstderr:")
        .map_or(output, |(stdout, _)| stdout);
    Some((notice, stdout))
}

pub(super) fn display_path(arguments: &serde_json::Value, cwd: &std::path::Path) -> String {
    string_arg(arguments, "path")
        .map(|path| compact_display_path(cwd, &path))
        .unwrap_or_default()
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

pub(super) fn compact_diff(diff: &str, include_file_headers: bool) -> Option<String> {
    let mut in_hunk = false;
    let mut lines = Vec::new();
    for line in diff.lines() {
        if in_hunk {
            if line.is_empty() {
                in_hunk = false;
                continue;
            }
            if line.starts_with("@@") || line.starts_with('\\') {
                continue;
            }
            let Some(content) = line.get(1..) else {
                continue;
            };
            match &line[..1] {
                "+" | "-" => lines.push(line.to_string()),
                " " => lines.push(content.to_string()),
                _ => {}
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ b/") {
            if include_file_headers {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(path.to_string());
            }
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(super) fn process_result_lines(content: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return vec!["process".into(), content.to_string()];
    };
    if value
        .get("stop_requested")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        if let Some(id) = value.get("process_id").and_then(|value| value.as_str()) {
            return vec!["process".into(), format!("stop requested: {id}")];
        }
    }
    let Some(command) = value.get("command").and_then(|value| value.as_str()) else {
        return vec!["process".into(), content.to_string()];
    };
    let mut lines = vec!["process".into(), command.to_string()];
    let state = value
        .get("state")
        .and_then(|value| value.as_str())
        .unwrap_or("running");
    let id = value
        .get("process_id")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let runtime = value
        .get("runtime_seconds")
        .and_then(|value| value.as_f64())
        .unwrap_or_default();
    let exit = value
        .get("exit_code")
        .and_then(|value| value.as_i64())
        .map_or_else(String::new, |code| format!(", exit code {code}"));
    lines.push(format!(
        "{} - {id} - {runtime:.1}s{exit}",
        state.replace('_', " ")
    ));
    if value.get("truncated").and_then(|value| value.as_bool()) == Some(true) {
        let cursor = value
            .get("first_cursor")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        lines.push(format!(
            "output before cursor {cursor} is no longer available"
        ));
    }
    if let Some(chunks) = value.get("chunks").and_then(|value| value.as_array()) {
        for chunk in chunks {
            let stream = chunk
                .get("stream")
                .and_then(|value| value.as_str())
                .unwrap_or("stdout");
            lines.push(format!("{stream}:"));
            lines.push(
                chunk
                    .get("text")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    if value
        .get("output_pending")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        let cursor = value
            .get("next_cursor")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        lines.push(format!("more output available at cursor {cursor}"));
    }
    if let Some(detail) = value
        .get("terminal_detail")
        .and_then(|value| value.as_str())
    {
        lines.push(format!("detail: {detail}"));
    }
    lines
}

pub(super) fn web_search_line(arguments: &serde_json::Value, content: &str) -> String {
    let status = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("answer")
                .and_then(|answer| answer.as_str())
                .map(str::to_string)
        })
        .map(|answer| {
            if answer.starts_with("No configured search provider") {
                "no live results".into()
            } else {
                pluralized(
                    answer
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .count(),
                    "result",
                    "stored",
                )
            }
        })
        .unwrap_or_else(|| "finished".into());
    let base = format!("web search: {status}");
    search_terms(arguments).map_or(base.clone(), |terms| format!("{base} for {terms}"))
}

pub(super) fn fetch_content_line(content: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return "fetch content finished".into();
    };
    if let Some(items) = value.get("items").and_then(|items| items.as_array()) {
        return format!(
            "fetch content: fetched {}",
            pluralized(items.len(), "item", "")
        );
    }
    if let Some(count) = value.get("itemCount").and_then(|count| count.as_u64()) {
        return format!(
            "fetch content: fetched {}",
            pluralized(count as usize, "item", "")
        );
    }
    if value.get("content").is_some() {
        let truncated = value
            .get("contentTruncated")
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false);
        return if truncated {
            "fetch content: fetched 1 item (truncated)".into()
        } else {
            "fetch content: fetched 1 item".into()
        };
    }
    "fetch content finished".into()
}

pub(super) fn get_search_content_line(content: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return "retrieved stored content".into();
    };
    if let Some(query) = value.get("query").and_then(|value| value.as_str()) {
        return format!("retrieved content for {}", quoted(query, 80));
    }
    let label = value
        .get("title")
        .and_then(|value| value.as_str())
        .or_else(|| value.get("url").and_then(|value| value.as_str()))
        .map(|value| truncate(value, 80))
        .unwrap_or_else(|| "stored content".into());
    format!("retrieved content: {label}")
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

pub(super) fn pluralized(count: usize, noun: &str, suffix: &str) -> String {
    let noun = if count == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    };
    if suffix.is_empty() {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun} {suffix}")
    }
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
