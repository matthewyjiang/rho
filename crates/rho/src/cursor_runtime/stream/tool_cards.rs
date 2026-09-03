//! Cursor Agent tool calls as [`ToolCard`]s.
//!
//! The wire body is `{ "<name>ToolCall": { args, result: { success | ... } } }`.
//! Cards keep Cursor's verbs (`Read`, `Shell`, `Edit`) and reuse the same
//! families and body types native Rho cards use. Unknown tools degrade to a
//! generic named card so new Cursor tools never break rendering.

#![allow(dead_code)] // Phase D session drain

use std::path::Path;

use serde_json::Value;

use rho_tools::{
    tool::compact_display_path,
    tool_card::{
        compact_diff_rows, diff_file_stats, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader,
        ToolStatus,
    },
};

use crate::claude_runtime::stream::{truncate_payload_lines, MAX_TOOL_BODY_LINES};

/// Cursor tool identity parsed from the wire key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorTool {
    Read,
    Edit,
    Delete,
    Shell,
    Grep,
    Glob,
    Ls,
    SemSearch,
    ReadLints,
    WebSearch,
    WebFetch,
    Mcp,
    Task,
    CreatePlan,
    Other,
}

impl CursorTool {
    fn from_key(key: &str) -> Self {
        match key {
            "readToolCall" => Self::Read,
            "editToolCall" => Self::Edit,
            "deleteToolCall" => Self::Delete,
            "shellToolCall" => Self::Shell,
            "grepToolCall" => Self::Grep,
            "globToolCall" => Self::Glob,
            "lsToolCall" => Self::Ls,
            "semSearchToolCall" => Self::SemSearch,
            "readLintsToolCall" => Self::ReadLints,
            "webSearchToolCall" => Self::WebSearch,
            "webFetchToolCall" | "fetchToolCall" => Self::WebFetch,
            "mcpToolCall" => Self::Mcp,
            "taskToolCall" => Self::Task,
            "createPlanToolCall" => Self::CreatePlan,
            _ => Self::Other,
        }
    }

    fn family(self) -> ToolFamily {
        match self {
            Self::Read | Self::Shell | Self::Grep | Self::Glob | Self::Ls | Self::SemSearch => {
                ToolFamily::FileCommand
            }
            Self::Edit | Self::Delete => ToolFamily::FileDiff,
            Self::WebSearch | Self::WebFetch => ToolFamily::Web,
            Self::Task => ToolFamily::Agent,
            Self::CreatePlan => ToolFamily::Form,
            Self::ReadLints | Self::Mcp | Self::Other => ToolFamily::Default,
        }
    }

    /// Verb shown in the card header.
    fn verb(self, key: &str) -> String {
        match self {
            Self::Read => "Read".into(),
            Self::Edit => "Edit".into(),
            Self::Delete => "Delete".into(),
            Self::Shell => "Shell".into(),
            Self::Grep => "Grep".into(),
            Self::Glob => "Glob".into(),
            Self::Ls => "LS".into(),
            Self::SemSearch => "SemSearch".into(),
            Self::ReadLints => "ReadLints".into(),
            Self::WebSearch => "WebSearch".into(),
            Self::WebFetch => "WebFetch".into(),
            Self::Mcp => "MCP".into(),
            Self::Task => "Task".into(),
            Self::CreatePlan => "CreatePlan".into(),
            // `fooBarToolCall` → `fooBar`
            Self::Other => key.strip_suffix("ToolCall").unwrap_or(key).to_string(),
        }
    }
}

/// A `tool_call/started` remembered until its `completed` frame arrives.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StartedCursorTool {
    kind: CursorTool,
    key: String,
    args: Option<Value>,
}

impl StartedCursorTool {
    pub(super) fn new(key: &str, args: Option<Value>) -> Self {
        Self {
            kind: CursorTool::from_key(key),
            key: key.to_string(),
            args: args.filter(|value| !value.is_null()),
        }
    }

    pub(super) fn verb(&self) -> String {
        self.kind.verb(&self.key)
    }

    fn header(&self, cwd: Option<&Path>) -> ToolHeader {
        let args = self.args.as_ref();
        match self.kind {
            CursorTool::Shell => ToolHeader::shell("$", string_field(args, &["command"])),
            CursorTool::Task => ToolHeader::status_first(
                self.verb(),
                string_field(args, &["description", "prompt"]).unwrap_or_default(),
            ),
            _ => ToolHeader::call(self.verb(), self.primary(cwd)),
        }
    }

    fn primary(&self, cwd: Option<&Path>) -> Option<String> {
        let args = self.args.as_ref();
        let primary = match self.kind {
            CursorTool::Read | CursorTool::Edit | CursorTool::Delete | CursorTool::Ls => {
                display_path_field(args, &["path"], cwd)
            }
            CursorTool::Glob => string_field(args, &["globPattern", "pattern"]),
            CursorTool::Grep => {
                let pattern = string_field(args, &["pattern"])?;
                Some(match display_path_field(args, &["path"], cwd) {
                    Some(path) => format!("{pattern}, {path}"),
                    None => pattern,
                })
            }
            CursorTool::SemSearch | CursorTool::WebSearch => {
                string_field(args, &["query"]).map(|query| quoted(&query, 80))
            }
            CursorTool::WebFetch => string_field(args, &["url"]).map(|url| truncate(&url, 80)),
            CursorTool::Mcp => string_field(args, &["name", "toolName"]),
            CursorTool::CreatePlan => string_field(args, &["title", "name"]),
            CursorTool::Shell | CursorTool::Task | CursorTool::ReadLints | CursorTool::Other => {
                None
            }
        };
        primary.filter(|value| !value.is_empty())
    }

    fn populate_running(&self, card: &mut ToolCard) {
        if let CursorTool::Shell = self.kind {
            push_shell_meta(card, self.args.as_ref());
        }
    }

    fn populate_finished(&self, card: &mut ToolCard, success: Option<&Value>) {
        let args = self.args.as_ref();
        match self.kind {
            CursorTool::Shell => {
                push_shell_meta(card, args);
                if let Some(code) = i64_field(success, &["exitCode"]) {
                    card.push_fact(ToolFact::Exit {
                        code,
                        duration_ms: u64_field(success, &["executionTime"]),
                    });
                }
                let output = string_field(success, &["interleavedOutput"])
                    .or_else(|| combined_stdio(success))
                    .unwrap_or_default();
                set_lines_body(card, &output);
            }
            CursorTool::Read => {
                if let Some(lines) = u64_field(success, &["totalLines"]) {
                    card.push_fact(count_fact("line", "lines", lines, None));
                }
            }
            CursorTool::Glob => {
                let files = string_list(success, "files");
                let count = u64_field(success, &["totalFiles"]).unwrap_or(files.len() as u64);
                card.push_fact(count_fact("file", "files", count, None));
                if !files.is_empty() {
                    set_lines_body(card, &files.join("\n"));
                }
            }
            CursorTool::Grep => push_grep_result(card, args, success),
            CursorTool::Edit => push_edit_result(card, success),
            CursorTool::Delete => {
                if let Some(path) = string_field(success, &["path"]) {
                    card.push_fact(ToolFact::Meta {
                        text: format!("deleted {path}"),
                    });
                }
            }
            CursorTool::Ls
            | CursorTool::SemSearch
            | CursorTool::ReadLints
            | CursorTool::WebSearch
            | CursorTool::WebFetch
            | CursorTool::Mcp
            | CursorTool::Task
            | CursorTool::CreatePlan
            | CursorTool::Other => {
                if let Some(message) = string_field(success, &["message", "text", "content"]) {
                    set_lines_body(card, &message);
                }
            }
        }
    }
}

pub(super) fn started_card(tool: &StartedCursorTool, cwd: Option<&Path>) -> ToolCard {
    let mut card = ToolCard::new(ToolStatus::Running, tool.kind.family(), tool.header(cwd));
    tool.populate_running(&mut card);
    card
}

/// Finished card from a `completed` frame's `result` object.
///
/// `result.success` populates tool facts. Any other single key (`error`,
/// `rejected`, ...) is treated as failure with the key's payload as detail.
pub(super) fn finished_card(
    tool: &StartedCursorTool,
    result: Option<&Value>,
    cwd: Option<&Path>,
) -> ToolCard {
    let outcome = ToolOutcome::from_result(result);
    let status = match outcome {
        ToolOutcome::Success(_) => ToolStatus::Ok,
        ToolOutcome::Failure { .. } | ToolOutcome::Missing => ToolStatus::Error,
    };
    let mut card = ToolCard::new(status, tool.kind.family(), tool.header(cwd));
    match outcome {
        ToolOutcome::Success(success) => tool.populate_finished(&mut card, Some(success)),
        ToolOutcome::Failure { key, detail } => {
            let summary = detail
                .as_ref()
                .and_then(|value| failure_summary(key, value))
                .unwrap_or_else(|| key.to_string());
            card.push_fact(ToolFact::Error {
                text: truncate(&summary, 160),
            });
        }
        ToolOutcome::Missing => card.push_fact(ToolFact::Error {
            text: "tool completed without a result".into(),
        }),
    }
    card
}

/// Shape of a completed tool result.
enum ToolOutcome<'a> {
    Success(&'a Value),
    Failure {
        key: &'a str,
        detail: Option<&'a Value>,
    },
    Missing,
}

impl<'a> ToolOutcome<'a> {
    fn from_result(result: Option<&'a Value>) -> Self {
        let Some(object) = result.and_then(Value::as_object) else {
            return Self::Missing;
        };
        if let Some(success) = object.get("success") {
            return Self::Success(success);
        }
        // Shell results carry a sibling `isBackground`; skip scalar keys when
        // looking for the outcome variant.
        match object
            .iter()
            .find(|(_, value)| value.is_object() || value.is_string())
        {
            Some((key, detail)) => Self::Failure {
                key,
                detail: Some(detail),
            },
            None => Self::Missing,
        }
    }
}

fn failure_summary(key: &str, detail: &Value) -> Option<String> {
    match detail {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => string_field(Some(detail), &["message", "error", "reason"])
            .or_else(|| Some(format!("{key}: {}", truncate(&detail.to_string(), 160)))),
        _ => None,
    }
}

fn push_shell_meta(card: &mut ToolCard, args: Option<&Value>) {
    if let Some(description) = string_field(args, &["description"]) {
        card.push_fact(ToolFact::Meta { text: description });
    }
    if let Some(timeout_ms) = u64_field(args, &["timeout"]) {
        card.push_fact(ToolFact::Timeout {
            seconds: Some(timeout_ms.saturating_add(999) / 1000),
        });
    }
    if args
        .and_then(|value| value.get("isBackground"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        card.push_fact(ToolFact::Meta {
            text: "background".into(),
        });
    }
}

fn combined_stdio(success: Option<&Value>) -> Option<String> {
    let stdout = string_field(success, &["stdout"]).unwrap_or_default();
    let stderr = string_field(success, &["stderr"]).unwrap_or_default();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => None,
        (false, true) => Some(stdout),
        (true, false) => Some(stderr),
        (false, false) => Some(format!("{stdout}\n{stderr}")),
    }
}

/// Grep results arrive as `workspaceResults: { <root>: { content: { matches:
/// [ { file, matches: [ { lineNumber, content } ] } ] } } }`.
fn push_grep_result(card: &mut ToolCard, args: Option<&Value>, success: Option<&Value>) {
    let mut lines = Vec::new();
    let mut files = 0_u64;
    let roots = success
        .and_then(|value| value.get("workspaceResults"))
        .and_then(Value::as_object);
    for root in roots.into_iter().flat_map(|map| map.values()) {
        let file_matches = root
            .pointer("/content/matches")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for file in file_matches {
            files += 1;
            let path = string_field(Some(file), &["file"]).unwrap_or_default();
            for hit in file
                .get("matches")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let line = u64_field(Some(hit), &["lineNumber"]).unwrap_or(0);
                let text = string_field(Some(hit), &["content"]).unwrap_or_default();
                lines.push(format!("{path}:{line}: {text}"));
            }
        }
    }
    let detail =
        (files > 0).then(|| format!("in {files} {}", if files == 1 { "file" } else { "files" }));
    card.push_fact(count_fact("match", "matches", lines.len() as u64, detail));
    set_lines_body(card, &lines.join("\n"));
    if let Some(pattern) = string_field(args, &["pattern"]) {
        let case_insensitive = args
            .and_then(|value| value.get("caseInsensitive"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        card.match_pattern = Some(pattern);
        card.match_literal = false;
        card.match_case_sensitive = !case_insensitive;
    }
}

/// Edit results carry a unified `diffString` plus `linesAdded` / `linesRemoved`.
fn push_edit_result(card: &mut ToolCard, success: Option<&Value>) {
    let diff = string_field(success, &["diffString"]);
    let counted = diff.as_deref().map(|diff| {
        diff_file_stats(diff)
            .into_iter()
            .fold((0, 0), |(added, removed), file| {
                (added + file.added, removed + file.removed)
            })
    });
    let added = u64_field(success, &["linesAdded"]).or_else(|| counted.map(|(added, _)| added));
    let removed =
        u64_field(success, &["linesRemoved"]).or_else(|| counted.map(|(_, removed)| removed));
    if let (Some(added), Some(removed)) = (added, removed) {
        card.push_fact(ToolFact::DiffStat {
            added,
            removed,
            path: None,
        });
    }
    if let Some(diff) = diff {
        let rows = compact_diff_rows(&diff, /*include_file_headers*/ false);
        if !rows.is_empty() {
            card.body = ToolBody::Diff(rows);
            return;
        }
    }
    if let Some(message) = string_field(success, &["message"]) {
        set_lines_body(card, &message);
    }
}

fn set_lines_body(card: &mut ToolCard, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    card.body = ToolBody::Lines(truncate_payload_lines(text, MAX_TOOL_BODY_LINES));
}

fn display_path_field(args: Option<&Value>, keys: &[&str], cwd: Option<&Path>) -> Option<String> {
    let path = string_field(args, keys)?;
    Some(match cwd {
        Some(cwd) => compact_display_path(cwd, &path),
        None => path,
    })
}

fn string_field(value: Option<&Value>, keys: &[&str]) -> Option<String> {
    let value = value?;
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    })
}

fn string_list(value: Option<&Value>, key: &str) -> Vec<String> {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .collect()
}

fn u64_field(value: Option<&Value>, keys: &[&str]) -> Option<u64> {
    let value = value?;
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
}

fn i64_field(value: Option<&Value>, keys: &[&str]) -> Option<i64> {
    let value = value?;
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
}

fn count_fact(singular: &str, plural: &str, value: u64, detail: Option<String>) -> ToolFact {
    ToolFact::Count {
        label: if value == 1 { singular } else { plural }.into(),
        value,
        detail,
    }
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
