//! Claude Code tool names as first-class [`ToolCard`]s.
//!
//! Keep Claude verbs (`Read`, `Bash`, `Glob`). Populate the same family,
//! header dialect, facts, and body types native Rho cards use. MCP tools
//! (`mcp__server__tool`) use the parsed tool name, a server provenance fact,
//! and argument summary. Unknown tools degrade to a named generic card.

use std::path::Path;

use serde_json::Value;

use rho_sdk::floor_char_boundary;

use rho_tools::{
    tool::compact_display_path,
    tool_card::{
        DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus,
    },
};

use crate::tools::mcp::{
    display::mcp_header_and_facts,
    exported_name::{parse_exported_name, ExportedNameDialect},
};

use super::format::{truncate_payload_lines, MAX_TOOL_BODY_LINES};
use super::types::MAX_TOOL_PAYLOAD_CHARS;

/// Raw `input_json_delta` assembly budget. Larger than the presentation cap
/// so a complete oversized object can be parsed, then reduced by
/// [`bounded_input`].
const MAX_INPUT_JSON_CHARS: usize = MAX_TOOL_PAYLOAD_CHARS.saturating_mul(16);

/// Claude tool identity parsed once from the wire name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeTool {
    Bash,
    Read,
    Write,
    Edit,
    NotebookEdit,
    Glob,
    Grep,
    Ls,
    WebSearch,
    WebFetch,
    Skill,
    Task,
    TodoWrite,
    AskUserQuestion,
    ExitPlanMode,
    EnterPlanMode,
    Mcp,
    Other,
}

impl ClaudeTool {
    fn from_name(name: &str) -> Self {
        match name {
            "Bash" => Self::Bash,
            "Read" => Self::Read,
            "Write" => Self::Write,
            "Edit" => Self::Edit,
            "NotebookEdit" => Self::NotebookEdit,
            "Glob" => Self::Glob,
            "Grep" => Self::Grep,
            "LS" => Self::Ls,
            "WebSearch" => Self::WebSearch,
            "WebFetch" => Self::WebFetch,
            "Skill" => Self::Skill,
            "Task" => Self::Task,
            "TodoWrite" => Self::TodoWrite,
            "AskUserQuestion" => Self::AskUserQuestion,
            "ExitPlanMode" => Self::ExitPlanMode,
            "EnterPlanMode" => Self::EnterPlanMode,
            _ if parse_exported_name(name, ExportedNameDialect::Conventional).is_some() => {
                Self::Mcp
            }
            _ => Self::Other,
        }
    }

    fn family(self) -> ToolFamily {
        match self {
            Self::Bash | Self::Read | Self::Glob | Self::Grep | Self::Ls => ToolFamily::FileCommand,
            Self::Edit | Self::Write | Self::NotebookEdit => ToolFamily::FileDiff,
            Self::WebSearch | Self::WebFetch => ToolFamily::Web,
            Self::Skill => ToolFamily::Skill,
            Self::Task => ToolFamily::Agent,
            Self::AskUserQuestion | Self::ExitPlanMode | Self::EnterPlanMode => ToolFamily::Form,
            Self::TodoWrite | Self::Mcp | Self::Other => ToolFamily::Default,
        }
    }
}

/// A Claude `tool_use` remembered until its `tool_result` arrives.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StartedClaudeTool {
    kind: ClaudeTool,
    name: String,
    pub(super) input: Option<Value>,
    /// Concatenated `input_json_delta` fragments for this tool.
    input_json: String,
}

impl StartedClaudeTool {
    pub(super) fn from_name_input(name: Option<&str>, input: Option<&Value>) -> Self {
        let name = clean_name(name);
        Self {
            kind: ClaudeTool::from_name(&name),
            name,
            input: bounded_input(input),
            input_json: String::new(),
        }
    }

    pub(super) fn from_block(block: &Value) -> Self {
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| block.get("tool_name").and_then(Value::as_str));
        Self::from_name_input(name, block.get("input"))
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    /// Fill input only when it is still missing. Later snapshots must not
    /// clobber a payload assembled from `input_json_delta`.
    pub(super) fn apply_input(&mut self, input: Option<&Value>) -> bool {
        if self.input.is_some() {
            return false;
        }
        let Some(input) = bounded_input(input) else {
            return false;
        };
        self.input = Some(input);
        true
    }

    /// Append a JSON fragment. Returns true when parsed input changed.
    pub(super) fn push_input_json(&mut self, fragment: &str) -> bool {
        if fragment.is_empty() {
            return false;
        }
        let room = MAX_INPUT_JSON_CHARS.saturating_sub(self.input_json.len());
        if room == 0 {
            return false;
        }
        if fragment.len() <= room {
            self.input_json.push_str(fragment);
        } else {
            let end = floor_char_boundary(fragment, room);
            if end == 0 {
                return false;
            }
            self.input_json.push_str(&fragment[..end]);
        }
        let Some(value) = parse_assembled_input(&self.input_json) else {
            return false;
        };
        let Some(input) = bounded_input(Some(&value)) else {
            return false;
        };
        if self.input.as_ref() == Some(&input) {
            return false;
        }
        self.input = Some(input);
        true
    }

    fn header(&self, cwd: Option<&Path>) -> ToolHeader {
        match self.kind {
            ClaudeTool::Bash => {
                ToolHeader::shell("$", string_field(self.input.as_ref(), &["command", "cmd"]))
            }
            ClaudeTool::Task => {
                let identity = string_field(self.input.as_ref(), &["subagent_type", "agent"])
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Task".into());
                let detail = string_field(self.input.as_ref(), &["description", "prompt"])
                    .unwrap_or_default();
                ToolHeader::status_first(identity, detail)
            }
            ClaudeTool::Mcp => match mcp_header_and_facts(
                self.name(),
                self.input.as_ref(),
                ExportedNameDialect::Conventional,
            ) {
                Some((header, _)) => header,
                None => ToolHeader::call(self.name(), self.primary(cwd)),
            },
            _ => ToolHeader::call(self.name(), self.primary(cwd)),
        }
    }

    fn primary(&self, cwd: Option<&Path>) -> Option<String> {
        let input = self.input.as_ref();
        let primary = match self.kind {
            ClaudeTool::Read | ClaudeTool::Write | ClaudeTool::Edit => {
                display_path_field(input, &["file_path", "path"], cwd)
            }
            ClaudeTool::NotebookEdit => {
                display_path_field(input, &["notebook_path", "file_path"], cwd)
            }
            ClaudeTool::Ls => display_path_field(input, &["path"], cwd),
            ClaudeTool::Glob => string_field(input, &["pattern"]),
            ClaudeTool::Grep => grep_primary(input, cwd),
            ClaudeTool::WebSearch => {
                string_field(input, &["query"]).map(|query| quoted(&query, 80))
            }
            ClaudeTool::WebFetch => string_field(input, &["url"]).map(|url| truncate(&url, 80)),
            ClaudeTool::Skill => string_field(input, &["skill", "command", "name"]),
            _ => None,
        };
        primary.filter(|value| !value.is_empty())
    }

    fn populate_running(&self, card: &mut ToolCard) {
        match self.kind {
            ClaudeTool::Bash => push_bash_meta(card, self.input.as_ref()),
            ClaudeTool::TodoWrite => push_todo_facts(card, self.input.as_ref()),
            ClaudeTool::Read | ClaudeTool::Write | ClaudeTool::Edit => {
                if let Some(range) = read_range_fact(self.input.as_ref()) {
                    card.push_fact(range);
                }
            }
            ClaudeTool::Mcp => push_mcp_facts(card, self.name(), self.input.as_ref()),
            _ => {}
        }
    }

    /// Tool-specific facts that must survive an error result. Exhaustive so a
    /// new tool decides intentionally whether its provenance outlives failure.
    fn populate_error(&self, card: &mut ToolCard) {
        match self.kind {
            ClaudeTool::Mcp => push_mcp_facts(card, self.name(), self.input.as_ref()),
            ClaudeTool::Bash
            | ClaudeTool::Read
            | ClaudeTool::Write
            | ClaudeTool::Edit
            | ClaudeTool::NotebookEdit
            | ClaudeTool::Glob
            | ClaudeTool::Grep
            | ClaudeTool::Ls
            | ClaudeTool::WebSearch
            | ClaudeTool::WebFetch
            | ClaudeTool::Skill
            | ClaudeTool::Task
            | ClaudeTool::TodoWrite
            | ClaudeTool::AskUserQuestion
            | ClaudeTool::ExitPlanMode
            | ClaudeTool::EnterPlanMode
            | ClaudeTool::Other => {}
        }
    }

    fn populate_finished(
        &self,
        card: &mut ToolCard,
        content_text: &str,
        tool_use_result: Option<&Value>,
    ) {
        let input = self.input.as_ref();
        match self.kind {
            ClaudeTool::Bash => {
                push_bash_meta(card, input);
                set_lines_body(card, content_text);
            }
            ClaudeTool::Read => {
                if let Some(range) = read_range_fact(input) {
                    card.push_fact(range);
                }
                let lines =
                    file_line_count(tool_use_result).or_else(|| count_nonempty_lines(content_text));
                if let Some(value) = lines {
                    card.push_fact(count_fact("line", "lines", value, None));
                }
            }
            ClaudeTool::Glob => {
                let files = filenames_from_result(tool_use_result);
                let value = u64_field(tool_use_result, &["numFiles", "num_files"])
                    .or_else(|| files.as_ref().map(|names| names.len() as u64))
                    .or_else(|| count_nonempty_lines(content_text));
                if let Some(value) = value {
                    card.push_fact(count_fact("file", "files", value, None));
                }
                if let Some(files) = files.filter(|names| !names.is_empty()) {
                    set_lines_body(card, &files.join("\n"));
                } else {
                    set_lines_body(card, content_text);
                }
            }
            ClaudeTool::Grep => push_grep_result(card, input, content_text, tool_use_result),
            ClaudeTool::Edit | ClaudeTool::Write | ClaudeTool::NotebookEdit => {
                push_diff_result(card, self.kind, input, content_text, tool_use_result);
            }
            ClaudeTool::WebSearch | ClaudeTool::WebFetch => {
                if let Some(value) = u64_field(tool_use_result, &["resultCount", "numResults"])
                    .or_else(|| count_nonempty_lines(content_text))
                {
                    card.push_fact(count_fact("result", "results", value, None));
                }
                set_lines_body(card, content_text);
            }
            ClaudeTool::TodoWrite => push_todo_facts(card, input),
            ClaudeTool::Mcp => {
                push_mcp_facts(card, self.name(), input);
                if let Some(value) = count_nonempty_lines(content_text) {
                    card.push_fact(count_fact("line", "lines", value, None));
                }
                set_lines_body(card, content_text);
            }
            _ => set_lines_body(card, content_text),
        }
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
    let fallback = StartedClaudeTool::from_name_input(None, None);
    let tool = tool.unwrap_or(&fallback);
    build_card(
        tool,
        ToolStatus::from_finished(ok),
        content_text,
        tool_use_result,
        cwd,
    )
}

fn build_card(
    tool: &StartedClaudeTool,
    status: ToolStatus,
    content_text: &str,
    tool_use_result: Option<&Value>,
    cwd: Option<&Path>,
) -> ToolCard {
    let mut card = ToolCard::new(status, tool.kind.family(), tool.header(cwd));
    match status {
        ToolStatus::Error => {
            tool.populate_error(&mut card);
            push_error_output(&mut card, content_text);
        }
        ToolStatus::Running => tool.populate_running(&mut card),
        ToolStatus::Ok | ToolStatus::Interrupted => {
            tool.populate_finished(&mut card, content_text, tool_use_result);
        }
    }
    card
}

fn push_mcp_facts(card: &mut ToolCard, name: &str, input: Option<&Value>) {
    let Some((_, facts)) = mcp_header_and_facts(name, input, ExportedNameDialect::Conventional)
    else {
        return;
    };
    for fact in facts {
        card.push_fact(fact);
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
    let match_lines = u64_field(tool_use_result, &["numMatches", "matchCount"])
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
    kind: ClaudeTool,
    input: Option<&Value>,
    content_text: &str,
    tool_use_result: Option<&Value>,
) {
    if let Some(rows) = diff_rows_from_result(tool_use_result, input, kind) {
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
    kind: ClaudeTool,
) -> Option<Vec<DiffRow>> {
    if let Some(rows) = structured_patch_rows(tool_use_result) {
        return Some(rows);
    }
    if matches!(kind, ClaudeTool::Write) {
        return write_result_rows(tool_use_result, input);
    }
    old_new_string_rows(input)
}

fn write_result_is_create(tool_use_result: Option<&Value>) -> bool {
    matches!(
        string_field(tool_use_result, &["type"]).as_deref(),
        Some("create")
    )
}

fn write_result_is_update(tool_use_result: Option<&Value>) -> bool {
    matches!(
        string_field(tool_use_result, &["type"]).as_deref(),
        Some("update")
    ) || string_field(tool_use_result, &["originalFile", "original_file"]).is_some()
}

fn write_result_rows(
    tool_use_result: Option<&Value>,
    input: Option<&Value>,
) -> Option<Vec<DiffRow>> {
    if write_result_is_update(tool_use_result) {
        return write_update_rows(tool_use_result, input);
    }
    if !write_result_is_create(tool_use_result) {
        return None;
    }
    let content = string_field(input, &["content"])
        .or_else(|| string_field(tool_use_result, &["content"]))?;
    let rows = line_rows(&content, DiffRowKind::Added);
    (!rows.is_empty()).then_some(rows)
}

fn write_update_rows(
    tool_use_result: Option<&Value>,
    input: Option<&Value>,
) -> Option<Vec<DiffRow>> {
    let old = string_field(tool_use_result, &["originalFile", "original_file"])?;
    let new = string_field(tool_use_result, &["content"])
        .or_else(|| string_field(input, &["content"]))?;
    replace_rows(&old, &new)
}

fn structured_patch_rows(tool_use_result: Option<&Value>) -> Option<Vec<DiffRow>> {
    let patch = tool_use_result
        .and_then(|value| value.get("structuredPatch"))
        .or_else(|| tool_use_result.and_then(|value| value.get("structured_patch")))?;
    let hunks = patch.as_array()?;
    if hunks.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for (index, hunk) in hunks.iter().enumerate() {
        if index > 0 {
            rows.push(DiffRow::new(DiffRowKind::Skip, None, "⋯"));
        }
        let lines = hunk.get("lines").and_then(Value::as_array)?;
        let mut old_line = u32_field(hunk, "oldStart").unwrap_or(1);
        let mut new_line = u32_field(hunk, "newStart").unwrap_or(1);
        for line in lines {
            let Some(text) = line.as_str() else {
                continue;
            };
            let (kind, content, line_no) = match text.as_bytes().first().copied() {
                Some(b'+') => (DiffRowKind::Added, tail_after_marker(text), new_line),
                Some(b'-') => (DiffRowKind::Removed, tail_after_marker(text), old_line),
                Some(b' ') => (DiffRowKind::Context, tail_after_marker(text), new_line),
                _ => continue,
            };
            match kind {
                DiffRowKind::Added => new_line = new_line.saturating_add(1),
                DiffRowKind::Removed => old_line = old_line.saturating_add(1),
                DiffRowKind::Context => {
                    old_line = old_line.saturating_add(1);
                    new_line = new_line.saturating_add(1);
                }
                _ => {}
            }
            rows.push(DiffRow::new(kind, Some(line_no), content));
        }
    }
    (!rows.is_empty()).then_some(rows)
}

fn old_new_string_rows(input: Option<&Value>) -> Option<Vec<DiffRow>> {
    let old = string_field(input, &["old_string", "oldString"])?;
    let new = string_field(input, &["new_string", "newString"])?;
    replace_rows(&old, &new)
}

fn replace_rows(old: &str, new: &str) -> Option<Vec<DiffRow>> {
    let mut rows = line_rows(old, DiffRowKind::Removed);
    rows.extend(line_rows(new, DiffRowKind::Added));
    (!rows.is_empty()).then_some(rows)
}

fn line_rows(text: &str, kind: DiffRowKind) -> Vec<DiffRow> {
    text.lines()
        .enumerate()
        .map(|(index, line)| {
            DiffRow::new(
                kind,
                Some(u32::try_from(index + 1).unwrap_or(u32::MAX)),
                line,
            )
        })
        .collect()
}

fn tail_after_marker(text: &str) -> &str {
    text.get(1..).unwrap_or_default()
}

fn u32_field(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn diff_row_stats(rows: &[DiffRow]) -> (u64, u64) {
    let mut added = 0;
    let mut removed = 0;
    for row in rows {
        match row.kind {
            DiffRowKind::Added => added += 1,
            DiffRowKind::Removed => removed += 1,
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
    /// Visible todo rows on the card. Extra items collapse to a count.
    const MAX_TODO_FACTS: usize = 10;
    for todo in todos.iter().take(MAX_TODO_FACTS) {
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
    if todos.len() > MAX_TODO_FACTS {
        card.push_fact(ToolFact::Meta {
            text: format!("{} more", todos.len() - MAX_TODO_FACTS),
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

/// Parse assembled `input_json_delta` text. Truncated objects keep leading
/// keys when a small closer produces valid JSON.
fn parse_assembled_input(raw: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(raw) {
        return Some(value);
    }
    for suffix in ["}", "\"}"] {
        if let Ok(value) = serde_json::from_str::<Value>(&format!("{raw}{suffix}")) {
            if value.is_object() {
                return Some(value);
            }
        }
    }
    None
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
