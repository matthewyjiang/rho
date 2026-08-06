//! Structured tool transcript cards for Call + Children rendering.
//!
//! Presenters build [`ToolCard`] values. Renderers draw them with multi-span
//! styles from the card structure (header, facts, body).

use serde::{Deserialize, Serialize};

/// Tool-family identity used for header verb color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFamily {
    FileCommand,
    FileDiff,
    Web,
    Skill,
    Form,
    Agent,
    Default,
}

/// Lifecycle status for the card marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
    Interrupted,
}

impl ToolStatus {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Ok => "✓",
            Self::Error => "✗",
            Self::Interrupted => "■",
        }
    }

    pub fn from_finished(ok: bool) -> Self {
        if ok {
            Self::Ok
        } else {
            Self::Error
        }
    }
}

/// Header layout dialect within the shared Call + Children grammar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolHeader {
    /// `verb(primary)` or bare `verb`.
    Call {
        verb: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        primary: Option<String>,
    },
    /// Shell prompt dialect: `$ command` / `PS command`.
    Shell {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    /// Agent-style `identity  detail`.
    StatusFirst { identity: String, detail: String },
}

impl ToolHeader {
    pub fn call(verb: impl Into<String>, primary: Option<String>) -> Self {
        Self::Call {
            verb: verb.into(),
            primary,
        }
    }

    pub fn shell(prompt: impl Into<String>, command: Option<String>) -> Self {
        Self::Shell {
            prompt: prompt.into(),
            command,
        }
    }

    pub fn status_first(identity: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::StatusFirst {
            identity: identity.into(),
            detail: detail.into(),
        }
    }
}

/// Structured child fact under a tool header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolFact {
    DiffStat {
        added: u64,
        removed: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Exit {
        code: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Count {
        label: String,
        value: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Meta {
        text: String,
    },
    Error {
        text: String,
    },
    Progress {
        completed: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
    },
    Text {
        text: String,
    },
}

/// What a diff row represents, which decides its sign and color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffRowKind {
    /// Unchanged line kept for context.
    Context,
    Added,
    Removed,
    /// File path heading in a multi-file diff.
    File,
    /// Gap between hunks (`⋯`).
    Skip,
    /// Non-content annotation (edit op locators, notices). Not a hunk gap.
    Meta,
}

impl DiffRowKind {
    /// Sign column text, so a color-stripped card still reads correctly.
    pub fn sign(self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Removed => "-",
            Self::Context | Self::File | Self::Skip | Self::Meta => " ",
        }
    }
}

/// One row of a compact diff body, with the line numbers for its gutter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    /// Line number: the new file's for context and additions, the old file's
    /// for removals. Absent for headings and for patch text without hunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub text: String,
}

impl DiffRow {
    pub fn new(kind: DiffRowKind, line: Option<u32>, text: impl Into<String>) -> Self {
        Self {
            kind,
            line,
            text: text.into(),
        }
    }

    /// Gutter number, sign and text as one string, for text-only surfaces.
    pub fn plain_text(&self) -> String {
        match self.kind {
            DiffRowKind::File | DiffRowKind::Skip | DiffRowKind::Meta => self.text.clone(),
            DiffRowKind::Context | DiffRowKind::Added | DiffRowKind::Removed => match self.line {
                Some(line) => format!("{line} {}{}", self.kind.sign(), self.text),
                None => format!("{}{}", self.kind.sign(), self.text),
            },
        }
    }
}

/// Optional expandable body content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "lines", rename_all = "snake_case")]
pub enum ToolBody {
    #[default]
    None,
    Lines(Vec<String>),
    /// Compact diff body with per-row line numbers and change kinds.
    Diff(Vec<DiffRow>),
}

impl ToolBody {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::None => true,
            Self::Lines(lines) => {
                lines.is_empty() || lines.iter().all(|line| line.trim().is_empty())
            }
            Self::Diff(rows) => rows.is_empty(),
        }
    }

    pub fn line_count(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Lines(lines) => lines.iter().map(|line| line.lines().count().max(1)).sum(),
            Self::Diff(rows) => rows.len(),
        }
    }

    pub fn is_diff(&self) -> bool {
        matches!(self, Self::Diff(_))
    }

    /// Body content as plain lines, for text-only surfaces and tests.
    pub fn plain_lines(&self) -> Vec<String> {
        match self {
            Self::None => Vec::new(),
            Self::Lines(lines) => lines.clone(),
            Self::Diff(rows) => rows.iter().map(DiffRow::plain_text).collect(),
        }
    }
}

/// Fact + body visibility for Call + Children rendering and expand/collapse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolCardDisplayPlan {
    /// How many leading facts to show.
    pub visible_facts: usize,
    /// How many body logical lines to show.
    pub visible_body_lines: usize,
    /// Child rows hidden (facts + body lines not shown).
    pub hidden_rows: usize,
    /// Whether Ctrl-O can toggle expand/collapse.
    pub expandable: bool,
    /// Show "ctrl+o to collapse" when expanded past the budget.
    pub show_collapse_prompt: bool,
}

/// Structured tool presentation for Call + Children rendering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCard {
    pub status: ToolStatus,
    pub family: ToolFamily,
    pub header: ToolHeader,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<ToolFact>,
    #[serde(default, skip_serializing_if = "ToolBody::is_empty")]
    pub body: ToolBody,
}

impl ToolCard {
    pub fn new(status: ToolStatus, family: ToolFamily, header: ToolHeader) -> Self {
        Self {
            status,
            family,
            header,
            facts: Vec::new(),
            body: ToolBody::None,
        }
    }

    pub fn with_facts(mut self, facts: Vec<ToolFact>) -> Self {
        self.facts = facts;
        self
    }

    pub fn with_body(mut self, body: ToolBody) -> Self {
        self.body = body;
        self
    }

    pub fn push_fact(&mut self, fact: ToolFact) {
        self.facts.push(fact);
    }

    pub fn push_notice_facts(&mut self, notices: &[String]) {
        for text in notices
            .iter()
            .flat_map(|notice| notice.lines())
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            self.facts.push(ToolFact::Meta {
                text: text.to_string(),
            });
        }
    }

    /// Plan fact+body visibility for a card.
    ///
    /// Rules:
    /// - Always keep header out of this plan (caller always draws header).
    /// - Facts and body form ONE child sequence under `max_lines` budget
    ///   (`max_lines.max(1)`).
    /// - When collapsed: apply budget across facts first, then body. Diff
    ///   bodies take the budget like any other body, so an edit shows its
    ///   opening rows without a keystroke.
    /// - When expanded: show all facts and all body lines.
    /// - `expandable` is true whenever the toggle has something to reveal or
    ///   hide: hidden rows when collapsed, or an over-budget card when expanded.
    /// - When collapsed and `hidden_rows > 0`, caller shows
    ///   `... {hidden_rows} more lines, ctrl+o to expand`.
    /// - When expanded and `show_collapse_prompt`, show `ctrl+o to collapse`.
    pub fn display_plan(&self, max_lines: usize, expanded: bool) -> ToolCardDisplayPlan {
        let budget = max_lines.max(1);
        let fact_count = self.facts.len();
        let body_lines = self.body.line_count();
        let total_children = fact_count + body_lines;

        if expanded {
            let show_collapse_prompt = total_children > budget;
            return ToolCardDisplayPlan {
                visible_facts: fact_count,
                visible_body_lines: body_lines,
                hidden_rows: 0,
                expandable: show_collapse_prompt,
                show_collapse_prompt,
            };
        }

        // Collapsed: budget covers facts first, then body lines.
        let visible_facts = fact_count.min(budget);
        let remaining = budget.saturating_sub(visible_facts);
        let visible_body_lines = body_lines.min(remaining);
        let hidden_rows = fact_count.saturating_sub(visible_facts)
            + body_lines.saturating_sub(visible_body_lines);
        ToolCardDisplayPlan {
            visible_facts,
            visible_body_lines,
            hidden_rows,
            expandable: hidden_rows > 0,
            show_collapse_prompt: false,
        }
    }

    pub fn header_text(&self) -> String {
        let marker = self.status.marker();
        match &self.header {
            ToolHeader::Call { verb, primary } => match primary {
                Some(primary) if !primary.is_empty() => format!("{marker} {verb}({primary})"),
                Some(_) | None => format!("{marker} {verb}"),
            },
            ToolHeader::Shell { prompt, command } => match command {
                Some(command) if !command.is_empty() => format!("{marker} {prompt} {command}"),
                Some(_) | None => format!("{marker} {prompt}"),
            },
            ToolHeader::StatusFirst { identity, detail } => {
                if detail.is_empty() {
                    format!("{marker} {identity}")
                } else {
                    format!("{marker} {identity}  {detail}")
                }
            }
        }
    }
}

impl ToolFact {
    /// Plain text for a fact, used by text-only surfaces and tests.
    pub fn plain_text(&self) -> String {
        match self {
            Self::DiffStat {
                added,
                removed,
                path,
            } => {
                let stats = format!("+{added} -{removed} lines");
                match path {
                    Some(path) if !path.is_empty() => format!("{stats} | {path}"),
                    Some(_) | None => stats,
                }
            }
            Self::Exit { code, duration_ms } => match duration_ms {
                Some(ms) => {
                    let secs = *ms as f64 / 1000.0;
                    format!("exit {code} · {secs:.1}s")
                }
                None => format!("exit {code}"),
            },
            Self::Count {
                label,
                value,
                detail,
            } => match detail {
                Some(detail) if !detail.is_empty() => format!("{value} {label} {detail}"),
                Some(_) | None => format!("{value} {label}"),
            },
            Self::Meta { text } | Self::Error { text } | Self::Text { text } => text.clone(),
            Self::Progress { completed, total } => match total {
                Some(total) => format!("{completed}/{total}"),
                None => format!("{completed}"),
            },
        }
    }
}

/// Per-file addition/removal counts extracted from a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFileStat {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// One file section from a unified diff parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedDiffFile {
    pub path: String,
    pub added: u64,
    pub removed: u64,
    pub rows: Vec<DiffRow>,
}

/// Kind of file change represented by a [`DiffCardFile`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffCardChange {
    /// Add or update content, including moves that keep body rows.
    #[default]
    Content,
    /// Delete a file. Body rows are usually empty during a streamed preview.
    Delete,
}

/// One file section ready for a FileDiff tool card body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffCardFile {
    pub path: String,
    /// Prior path when this section is a move/rename.
    pub source_path: Option<String>,
    pub change: DiffCardChange,
    /// Present when the section has known addition/removal counts.
    pub stats: Option<(u64, u64)>,
    pub rows: Vec<DiffRow>,
}

impl DiffCardFile {
    /// Path shown in headers and multi-file body headings.
    pub fn display_path(&self) -> String {
        match &self.source_path {
            Some(source) if source != &self.path => format!("{source} → {}", self.path),
            _ => self.path.clone(),
        }
    }
}

impl From<ParsedDiffFile> for DiffCardFile {
    fn from(file: ParsedDiffFile) -> Self {
        Self {
            path: file.path,
            source_path: None,
            change: DiffCardChange::Content,
            stats: Some((file.added, file.removed)),
            rows: file.rows,
        }
    }
}

/// Parse a unified diff once; callers derive stats, body rows, and path counts.
pub fn parse_unified_diff(diff: &str) -> Vec<ParsedDiffFile> {
    let mut files = Vec::new();
    let mut current: Option<OpenDiffFile> = None;
    let mut pending_old_path: Option<String> = None;
    let mut in_hunk = false;

    for line in diff.lines() {
        if should_exit_hunk(line) {
            in_hunk = false;
        }

        if let Some(path) = minus_file_path(line) {
            pending_old_path = Some(path);
            continue;
        }

        if let Some(new_path) = plus_file_path(line) {
            if let Some(file) = current.take() {
                files.push(file.finish());
            }
            let old_path = pending_old_path.take();
            current = Some(OpenDiffFile {
                path: display_diff_path(old_path.as_deref(), &new_path),
                added: 0,
                removed: 0,
                rows: Vec::new(),
                next_old: 1,
                next_new: 1,
                seen_hunk: false,
            });
            continue;
        }

        if let Some((old_start, new_start)) = hunk_starts(line) {
            in_hunk = true;
            if let Some(file) = current.as_mut() {
                // A second hunk means skipped lines; mark the gap so the row
                // numbers do not look like a contiguous run.
                if file.seen_hunk {
                    file.rows.push(DiffRow::new(DiffRowKind::Skip, None, "⋯"));
                }
                file.seen_hunk = true;
                file.next_old = old_start;
                file.next_new = new_start;
            }
            continue;
        }

        if !in_hunk || line.is_empty() || line.starts_with('\\') {
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };
        let Some(marker) = line.as_bytes().first().copied() else {
            continue;
        };
        let content = line.get(1..).unwrap_or_default().to_string();
        match marker {
            b'+' => {
                file.added += 1;
                file.rows.push(DiffRow::new(
                    DiffRowKind::Added,
                    Some(file.next_new),
                    content,
                ));
                file.next_new += 1;
            }
            b'-' => {
                file.removed += 1;
                file.rows.push(DiffRow::new(
                    DiffRowKind::Removed,
                    Some(file.next_old),
                    content,
                ));
                file.next_old += 1;
            }
            b' ' => {
                file.rows.push(DiffRow::new(
                    DiffRowKind::Context,
                    Some(file.next_new),
                    content,
                ));
                file.next_old += 1;
                file.next_new += 1;
            }
            _ => {}
        }
    }

    if let Some(file) = current {
        files.push(file.finish());
    }
    files
}

/// Extract per-file `+N -M` stats from a unified diff.
pub fn diff_file_stats(diff: &str) -> Vec<DiffFileStat> {
    parse_unified_diff(diff)
        .into_iter()
        .map(|file| DiffFileStat {
            path: file.path,
            added: file.added,
            removed: file.removed,
        })
        .collect()
}

/// Collapse a unified diff to numbered add/remove/context rows for the card body.
pub fn compact_diff_rows(diff: &str, include_file_headers: bool) -> Vec<DiffRow> {
    compact_diff_rows_from_files(&parse_unified_diff(diff), include_file_headers)
}

/// Build compact body rows from an already-parsed unified diff.
pub fn compact_diff_rows_from_files(
    files: &[ParsedDiffFile],
    include_file_headers: bool,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for file in files {
        push_compact_diff_section(&mut rows, &file.path, &file.rows, include_file_headers);
    }
    rows
}

/// Build compact body rows from card-facing file sections.
pub fn compact_diff_rows_from_card_files(
    files: &[DiffCardFile],
    include_file_headers: bool,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for file in files {
        push_compact_diff_section(
            &mut rows,
            file.display_path(),
            &file.rows,
            include_file_headers,
        );
    }
    rows
}

fn push_compact_diff_section(
    rows: &mut Vec<DiffRow>,
    path: impl Into<String>,
    file_rows: &[DiffRow],
    include_file_headers: bool,
) {
    if include_file_headers {
        rows.push(DiffRow::new(DiffRowKind::File, None, path));
    }
    rows.extend(file_rows.iter().cloned());
}

#[derive(Clone, Debug)]
struct OpenDiffFile {
    path: String,
    added: u64,
    removed: u64,
    rows: Vec<DiffRow>,
    next_old: u32,
    next_new: u32,
    seen_hunk: bool,
}

impl OpenDiffFile {
    fn finish(self) -> ParsedDiffFile {
        ParsedDiffFile {
            path: self.path,
            added: self.added,
            removed: self.removed,
            rows: self.rows,
        }
    }
}

/// Prefer the surviving path: new path unless the file was deleted.
fn display_diff_path(old_path: Option<&str>, new_path: &str) -> String {
    if new_path == "/dev/null" {
        old_path.unwrap_or(new_path).to_string()
    } else {
        new_path.to_string()
    }
}

/// Old and new starting line numbers from an `@@ -a,b +c,d @@` header.
fn hunk_starts(line: &str) -> Option<(u32, u32)> {
    let body = line.strip_prefix("@@ ")?;
    let (ranges, _) = body.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let start = |range: &str, sign: char| -> Option<u32> {
        range
            .strip_prefix(sign)?
            .split(',')
            .next()?
            .parse::<u32>()
            .ok()
    };
    Some((start(old, '-')?, start(new, '+')?))
}

fn strip_diff_path(line: &str, prefixes: &[&str]) -> Option<String> {
    for prefix in prefixes {
        if let Some(path) = line.strip_prefix(prefix) {
            let path = path.split('\t').next().unwrap_or(path).trim();
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
    }
    None
}

fn minus_file_path(line: &str) -> Option<String> {
    if line.starts_with("--- /dev/null") {
        return Some("/dev/null".into());
    }
    strip_diff_path(line, &["--- a/", "--- b/", "--- "])
}

fn plus_file_path(line: &str) -> Option<String> {
    if line.starts_with("+++ /dev/null") {
        return Some("/dev/null".into());
    }
    strip_diff_path(line, &["+++ b/", "+++ a/", "+++ "])
}

/// Leave hunk mode at file boundaries so multi-file diffs need no blank separator.
fn should_exit_hunk(line: &str) -> bool {
    line.is_empty() || line.starts_with("diff --git") || is_file_header_line(line)
}

fn is_file_header_line(line: &str) -> bool {
    line.starts_with("--- ") || line.starts_with("+++ ")
}

#[cfg(test)]
#[path = "tool_card_tests.rs"]
mod tests;
