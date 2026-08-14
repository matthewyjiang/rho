//! Claude Code tool names as first-class [`ToolCard`]s.
//!
//! Keep Claude verbs (`Read`, `Bash`, `Glob`). Populate the same family,
//! header dialect, facts, and body types native Rho cards use. Unknown and
//! MCP tools degrade to a named generic card.

use std::path::Path;

use serde_json::Value;

use rho_tools::{
    tool::compact_display_path,
    tool_card::{
        compact_diff_rows, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus,
    },
};

use super::format::{truncate_payload_lines, MAX_TOOL_BODY_LINES};
use super::types::MAX_TOOL_PAYLOAD_CHARS;

/// A Claude `tool_use` remembered until its `tool_result` arrives.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StartedClaudeTool {
    pub(super) name: String,
    pub(super) input: Option<Value>,
}

impl StartedClaudeTool {
    pub(super) fn from_name_input(name: Option<&str>, input: Option<&Value>) -> Self {
        Self {
            name: clean_name(name),
            input: bounded_input(input),
        }
    }

    pub(super) fn from_block(block: &Value) -> Self {
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| block.get("tool_name").and_then(Value::as_str));
        Self::from_name_input(name, block.get("input"))
    }

    /// Replace empty/`{}` input with a later complete payload. Returns true
    /// when the stored input changed.
    pub(super) fn apply_input(&mut self, input: Option<&Value>) -> bool {
        let Some(input) = bounded_input(input) else {
            return false;
        };
        if self.input.as_ref() == Some(&input) {
            return false;
        }
        self.input = Some(input);
        true
    }
}

pub(super) fn started_card(tool: &StartedClaudeTool, cwd: Option<&Path>) -> ToolCard {
    build_card(tool, ToolStatus::Running, "", None, cwd)
}

pub(super) fn finished_card(
    tool: Option<&StartedClaudeTool>,
    ok: bool,
    content_text: &str,
    tool_use_result: Option<&Value>,
    cwd: Option<&Path>,
) -> ToolCard {
    let fallback = StartedClaudeTool {
        name: "tool".into(),
        input: None,
    };
    let tool = tool.unwrap_or(&fallback);
    let status = ToolStatus::from_finished(ok);
    build_card(tool, status, content_text, tool_use_result, cwd)
}

fn build_card(
    tool: &StartedClaudeTool,
    status: ToolStatus,
    content_text: &str,
    tool_use_result: Option<&Value>,
    cwd: Option<&Path>,
) -> ToolCard {
    let input = tool.input.as_ref();
    let mut card = ToolCard::new(status, family_for(&tool.name), header_for(tool, cwd));
    if status == ToolStatus::Error {
        push_error_output(&mut card, content_text);
        return card;
    }
    if status == ToolStatus::Running {
        populate_running(&mut card, tool);
        return card;
    }
    populate_finished(&mut card, tool, input, content_text, tool_use_result, cwd);
    card
}

fn family_for(name: &str) -> ToolFamily {
    match name {
        "Bash" | "Read" | "Glob" | "Grep" | "LS" => ToolFamily::FileCommand,
        "Edit" | "Write" | "NotebookEdit" => ToolFamily::FileDiff,
        "WebSearch" | "WebFetch" => ToolFamily::Web,
        "Skill" => ToolFamily::Skill,
        "Task" => ToolFamily::Agent,
        "AskUserQuestion" | "ExitPlanMode" | "EnterPlanMode" => ToolFamily::Form,
        _ => ToolFamily::Default,
    }
}

fn header_for(tool: &StartedClaudeTool, cwd: Option<&Path>) -> ToolHeader {
    let input = tool.input.as_ref();
    match tool.name.as_str() {
        "Bash" => ToolHeader::shell("$", string_field(input, &["command", "cmd"])),
        "Task" => {
            let identity = string_field(input, &["subagent_type", "agent"])
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Task".into());
            let detail = string_field(input, &["description", "prompt"]).unwrap_or_default();
            ToolHeader::status_first(identity, detail)
        }
        _ => ToolHeader::call(&tool.name, primary_for(&tool.name, input, cwd)),
    }
}

fn primary_for(name: &str, input: Option<&Value>, cwd: Option<&Path>) -> Option<String> {
    let primary = match name {
        "Read" | "Write" | "Edit" => display_path_field(input, &["file_path", "path"], cwd),
        "NotebookEdit" => display_path_field(input, &["notebook_path", "file_path"], cwd),
        "LS" => display_path_field(input, &["path"], cwd),
        "Glob" => string_field(input, &["pattern"]),
        "Grep" => grep_primary(input, cwd),
        "WebSearch" => string_field(input, &["query"]).map(|query| quoted(&query, 80)),
        "WebFetch" => string_field(input, &["url"]).map(|url| truncate(&url, 80)),
        "Skill" => string_field(input, &["skill", "command", "name"]),
        _ => None,
    };
    primary.filter(|value| !value.is_empty())
}

fn populate_running(card: &mut ToolCard, tool: &StartedClaudeTool) {
    match tool.name.as_str() {
        "Bash" => push_bash_meta(card, tool.input.as_ref()),
        "TodoWrite" => push_todo_facts(card, tool.input.as_ref()),
        "Read" | "Write" | "Edit" => {
            if let Some(range) = read_range_fact(tool.input.as_ref()) {
                card.push_fact(range);
            }
        }
        _ => {}
    }
}

fn populate_finished(
    card: &mut ToolCard,
    tool: &StartedClaudeTool,
    input: Option<&Value>,
    content_text: &str,
    tool_use_result: Option<&Value>,
    cwd: Option<&Path>,
) {
    match tool.name.as_str() {
        "Bash" => {
            push_bash_meta(card, input);
            set_lines_body(card, content_text);
        }
        "Read" => {
            if let Some(range) = read_range_fact(input) {
                card.push_fact(range);
            }
            let lines =
                file_line_count(tool_use_result).or_else(|| count_nonempty_lines(content_text));
            if let Some(value) = lines {
                card.push_fact(count_fact("line", "lines", value, None));
            }
        }
        "Glob" => {
            let files = filenames_from_result(tool_use_result);
            let value = u64_field(tool_use_result, &["numFiles", "num_files"])
                .or_else(|| files.as_ref().map(|names| names.len() as u64))
                .or_else(|| count_nonempty_lines(content_text));
            if let Some(value) = value {
                card.push_fact(count_fact("file", "files", value, None));
            }
            if let Some(files) = files {
                if !files.is_empty() {
                    card.body = ToolBody::Lines(truncate_payload_lines(
                        &files.join("\n"),
                        MAX_TOOL_BODY_LINES,
                    ));
                }
            } else {
                set_lines_body(card, content_text);
            }
        }
        "Grep" => {
            push_grep_result(card, input, content_text, tool_use_result);
        }
        "Edit" | "Write" | "NotebookEdit" => {
            push_diff_result(card, tool, input, content_text, tool_use_result, cwd);
        }
        "WebSearch" | "WebFetch" => {
            if let Some(value) = u64_field(tool_use_result, &["resultCount", "numResults"])
                .or_else(|| count_nonempty_lines(content_text))
            {
                card.push_fact(count_fact("result", "results", value, None));
            }
            set_lines_body(card, content_text);
        }
        "TodoWrite" => push_todo_facts(card, input),
        "Skill" | "Task" | "LS" => set_lines_body(card, content_text),
        _ => set_lines_body(card, content_text),
    }
}

fn push_bash_meta(card: &mut ToolCard, input: Option<&Value>) {
    if let Some(description) = string_field(input, &["description"]) {
        card.push_fact(ToolFact::Meta { text: description });
    }
    if let Some(timeout_ms) = u64_field(input, &["timeout"]) {
        card.push_fact(ToolFact::Timeout {
            seconds: Some(timeout_ms.saturating_add(999) / 1000),
        });
    }
    if input
        .and_then(|value| value.get("run_in_background"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        card.push_fact(ToolFact::Meta {
            text: "background".into(),
        });
    }
}

fn push_grep_result(
    card: &mut ToolCard,
    input: Option<&Value>,
    content_text: &str,
    tool_use_result: Option<&Value>,
) {
    let match_lines = u64_field(tool_use_result, &["numMatches", "matchCount", "modeCount"])
        .or_else(|| count_nonempty_lines(content_text))
        .unwrap_or(0);
    let file_count = u64_field(tool_use_result, &["numFiles", "fileCount"]);
    let detail =
        file_count.map(|files| format!("in {files} {}", if files == 1 { "file" } else { "files" }));
    card.push_fact(count_fact("match", "matches", match_lines, detail));
    set_lines_body(card, content_text);
    if let Some(pattern) = string_field(input, &["pattern"]) {
        let case_sensitive = !input
            .and_then(|value| {
                value
                    .get("-i")
                    .or_else(|| value.get("case_insensitive"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false);
        card.match_pattern = Some(pattern);
        card.match_literal = false;
        card.match_case_sensitive = case_sensitive;
    }
}

fn push_diff_result(
    card: &mut ToolCard,
    tool: &StartedClaudeTool,
    input: Option<&Value>,
    content_text: &str,
    tool_use_result: Option<&Value>,
    cwd: Option<&Path>,
) {
    let path = display_path_field(input, &["file_path", "path", "notebook_path"], cwd);
    if let Some(rows) =
        diff_rows_from_result(tool_use_result, input, tool.name.as_str(), path.as_deref())
    {
        let (added, removed) = diff_row_stats(&rows);
        card.push_fact(ToolFact::DiffStat {
            added,
            removed,
            path: None,
        });
        if !rows.is_empty() {
            card.body = ToolBody::Diff(rows);
        }
        return;
    }
    set_lines_body(card, content_text);
}

fn diff_rows_from_result(
    tool_use_result: Option<&Value>,
    input: Option<&Value>,
    name: &str,
    path: Option<&str>,
) -> Option<Vec<rho_tools::tool_card::DiffRow>> {
    if let Some(rows) = structured_patch_rows(tool_use_result, path) {
        return Some(rows);
    }
    if name != "Write" {
        return old_new_string_rows(input);
    }
    if write_result_is_update(tool_use_result) {
        return write_update_rows(tool_use_result, input, path);
    }
    let content = string_field(input, &["content"])
        .or_else(|| string_field(tool_use_result, &["content"]))?;
    write_create_rows(&content, path.unwrap_or("file"))
}

fn write_result_is_update(tool_use_result: Option<&Value>) -> bool {
    matches!(
        string_field(tool_use_result, &["type"]).as_deref(),
        Some("update")
    ) || string_field(tool_use_result, &["originalFile", "original_file"]).is_some()
}

fn write_update_rows(
    tool_use_result: Option<&Value>,
    input: Option<&Value>,
    path: Option<&str>,
) -> Option<Vec<rho_tools::tool_card::DiffRow>> {
    let old = string_field(tool_use_result, &["originalFile", "original_file"])?;
    let new = string_field(tool_use_result, &["content"])
        .or_else(|| string_field(input, &["content"]))?;
    replace_rows(&old, &new, path.unwrap_or("file"))
}

fn write_create_rows(content: &str, path: &str) -> Option<Vec<rho_tools::tool_card::DiffRow>> {
    let line_count = content.lines().count().max(1);
    let mut unified = format!("--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{line_count} @@\n");
    for line in content.lines() {
        unified.push('+');
        unified.push_str(line);
        unified.push('\n');
    }
    let rows = compact_diff_rows(&unified, /*include_file_headers*/ false);
    (!rows.is_empty()).then_some(rows)
}

fn structured_patch_rows(
    tool_use_result: Option<&Value>,
    path: Option<&str>,
) -> Option<Vec<rho_tools::tool_card::DiffRow>> {
    let patch = tool_use_result
        .and_then(|value| value.get("structuredPatch"))
        .or_else(|| tool_use_result.and_then(|value| value.get("structured_patch")))?;
    let hunks = patch.as_array()?;
    if hunks.is_empty() {
        return None;
    }
    let path = path.unwrap_or("file");
    let mut unified = format!("--- a/{path}\n+++ b/{path}\n");
    for hunk in hunks {
        let lines = hunk.get("lines").and_then(Value::as_array)?;
        let old_start = hunk.get("oldStart").and_then(Value::as_u64).unwrap_or(1);
        let new_start = hunk.get("newStart").and_then(Value::as_u64).unwrap_or(1);
        let old_lines = hunk
            .get("oldLines")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| count_hunk_side(lines, /*added*/ false));
        let new_lines = hunk
            .get("newLines")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| count_hunk_side(lines, /*added*/ true));
        unified.push_str(&format!(
            "@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n"
        ));
        for line in lines {
            if let Some(text) = line.as_str() {
                unified.push_str(text);
                unified.push('\n');
            }
        }
    }
    let rows = compact_diff_rows(&unified, /*include_file_headers*/ false);
    (!rows.is_empty()).then_some(rows)
}

fn old_new_string_rows(input: Option<&Value>) -> Option<Vec<rho_tools::tool_card::DiffRow>> {
    let old = string_field(input, &["old_string", "oldString"])?;
    let new = string_field(input, &["new_string", "newString"])?;
    replace_rows(&old, &new, "file")
}

fn replace_rows(old: &str, new: &str, path: &str) -> Option<Vec<rho_tools::tool_card::DiffRow>> {
    let old_count = old.lines().count().max(1);
    let new_count = new.lines().count().max(1);
    let mut unified = format!("--- a/{path}\n+++ b/{path}\n@@ -1,{old_count} +1,{new_count} @@\n");
    for line in old.lines() {
        unified.push('-');
        unified.push_str(line);
        unified.push('\n');
    }
    for line in new.lines() {
        unified.push('+');
        unified.push_str(line);
        unified.push('\n');
    }
    let rows = compact_diff_rows(&unified, /*include_file_headers*/ false);
    (!rows.is_empty()).then_some(rows)
}

fn count_hunk_side(lines: &[Value], added: bool) -> u64 {
    lines
        .iter()
        .filter_map(Value::as_str)
        .filter(|line| {
            let marker = line.as_bytes().first().copied();
            match marker {
                Some(b'+') => added,
                Some(b'-') => !added,
                Some(b' ') | None => true,
                _ => false,
            }
        })
        .count() as u64
}

fn diff_row_stats(rows: &[rho_tools::tool_card::DiffRow]) -> (u64, u64) {
    let mut added = 0;
    let mut removed = 0;
    for row in rows {
        match row.kind {
            rho_tools::tool_card::DiffRowKind::Added => added += 1,
            rho_tools::tool_card::DiffRowKind::Removed => removed += 1,
            _ => {}
        }
    }
    (added, removed)
}

fn push_todo_facts(card: &mut ToolCard, input: Option<&Value>) {
    let Some(todos) = input
        .and_then(|value| value.get("todos"))
        .and_then(Value::as_array)
    else {
        return;
    };
    const MAX_TODOS: usize = 10;
    for todo in todos.iter().take(MAX_TODOS) {
        let text = string_field(Some(todo), &["content", "activeForm"]).unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let status = string_field(Some(todo), &["status"]).unwrap_or_default();
        let marker = match status.as_str() {
            "completed" | "complete" => "☑",
            "in_progress" | "in-progress" => "◐",
            _ => "☐",
        };
        card.push_fact(ToolFact::Text {
            text: format!("{marker} {text}"),
        });
    }
    if todos.len() > MAX_TODOS {
        card.push_fact(ToolFact::Meta {
            text: format!("{} more", todos.len() - MAX_TODOS),
        });
    }
}

fn read_range_fact(input: Option<&Value>) -> Option<ToolFact> {
    let offset = u64_field(input, &["offset"]);
    let limit = u64_field(input, &["limit"]);
    match (offset, limit) {
        (None, None) => None,
        (offset, limit) => {
            let start = offset.unwrap_or(1);
            let text = match limit {
                Some(limit) => format!("{start}-{}", start.saturating_add(limit).saturating_sub(1)),
                None => format!("{start}-"),
            };
            Some(ToolFact::Meta { text })
        }
    }
}

fn file_line_count(tool_use_result: Option<&Value>) -> Option<u64> {
    tool_use_result
        .and_then(|value| value.get("file"))
        .and_then(|file| {
            file.get("numLines")
                .or_else(|| file.get("totalLines"))
                .or_else(|| file.get("num_lines"))
        })
        .and_then(Value::as_u64)
}

fn filenames_from_result(tool_use_result: Option<&Value>) -> Option<Vec<String>> {
    let names = tool_use_result
        .and_then(|value| value.get("filenames"))
        .or_else(|| tool_use_result.and_then(|value| value.get("files")))?
        .as_array()?;
    let files = names
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    Some(files)
}

fn set_lines_body(card: &mut ToolCard, content_text: &str) {
    if content_text.trim().is_empty() {
        return;
    }
    card.body = ToolBody::Lines(truncate_payload_lines(content_text, MAX_TOOL_BODY_LINES));
}

fn push_error_output(card: &mut ToolCard, content: &str) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }
    let lines = truncate_payload_lines(trimmed, MAX_TOOL_BODY_LINES);
    let Some(first) = lines.first() else {
        return;
    };
    let summary = truncate(first.trim(), 160);
    card.push_fact(ToolFact::Error {
        text: summary.clone(),
    });
    if lines.len() > 1 {
        card.body = ToolBody::Lines(lines[1..].to_vec());
    }
}

fn grep_primary(input: Option<&Value>, cwd: Option<&Path>) -> Option<String> {
    let pattern = string_field(input, &["pattern"])?;
    match display_path_field(input, &["path"], cwd) {
        Some(path) => Some(format!("{pattern}, {path}")),
        None => Some(pattern),
    }
}

fn display_path_field(input: Option<&Value>, keys: &[&str], cwd: Option<&Path>) -> Option<String> {
    let path = string_field(input, keys)?;
    Some(match cwd {
        Some(cwd) => compact_display_path(cwd, &path),
        None => path,
    })
}

fn string_field(input: Option<&Value>, keys: &[&str]) -> Option<String> {
    let input = input?;
    keys.iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn u64_field(input: Option<&Value>, keys: &[&str]) -> Option<u64> {
    let input = input?;
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_u64))
}

fn count_fact(singular: &str, plural: &str, value: u64, detail: Option<String>) -> ToolFact {
    ToolFact::Count {
        label: if value == 1 {
            singular.into()
        } else {
            plural.into()
        },
        value,
        detail,
    }
}

fn count_nonempty_lines(content: &str) -> Option<u64> {
    let count = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    (count > 0).then_some(count)
}

/// Fields at or under this encoded size are presentation metadata and are
/// kept even when a sibling body field overflows the payload budget.
const SMALL_INPUT_FIELD_CHARS: usize = 512;

fn bounded_input(input: Option<&Value>) -> Option<Value> {
    let value = input.filter(|value| !value.is_null())?;
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        return None;
    }
    let serialized = serde_json::to_string(value).ok()?;
    if serialized.len() <= MAX_TOOL_PAYLOAD_CHARS {
        return Some(value.clone());
    }
    bound_oversized_object(value)
}

fn bound_oversized_object(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let mut kept = serde_json::Map::new();
    let mut large_strings = Vec::new();
    for (key, field) in object {
        if let Value::String(text) = field {
            if encoded_len(field) > SMALL_INPUT_FIELD_CHARS {
                large_strings.push((key.clone(), text.clone()));
                continue;
            }
        }
        if encoded_len(field) <= SMALL_INPUT_FIELD_CHARS && object_fits(&kept, key, field) {
            kept.insert(key.clone(), field.clone());
        }
    }
    for (key, text) in large_strings {
        if let Some(field) = largest_fitting_string(&kept, &key, &text) {
            kept.insert(key, field);
        }
    }
    (!kept.is_empty()).then_some(Value::Object(kept))
}

/// Longest prefix of `text` whose JSON object still fits the payload budget.
fn largest_fitting_string(
    kept: &serde_json::Map<String, Value>,
    key: &str,
    text: &str,
) -> Option<Value> {
    let full = Value::String(text.to_string());
    if object_fits(kept, key, &full) {
        return Some(full);
    }
    let chars = text.chars().collect::<Vec<_>>();
    let mut lo = 0;
    let mut hi = chars.len();
    let mut best = None;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let prefix = chars[..mid].iter().collect::<String>();
        let field = Value::String(prefix);
        if object_fits(kept, key, &field) {
            best = Some(field);
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    best
}

fn encoded_len(value: &Value) -> usize {
    serde_json::to_string(value)
        .map(|encoded| encoded.len())
        .unwrap_or(usize::MAX)
}

fn encoded_object_len(object: &serde_json::Map<String, Value>) -> usize {
    encoded_len(&Value::Object(object.clone()))
}

fn object_fits(kept: &serde_json::Map<String, Value>, key: &str, field: &Value) -> bool {
    let mut probe = kept.clone();
    probe.insert(key.to_string(), field.clone());
    encoded_object_len(&probe) <= MAX_TOOL_PAYLOAD_CHARS
}

fn clean_name(name: Option<&str>) -> String {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string()
}

fn quoted(text: &str, max_chars: usize) -> String {
    format!("\"{}\"", truncate(text, max_chars.saturating_sub(2)))
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "tool_cards_tests.rs"]
mod tests;
