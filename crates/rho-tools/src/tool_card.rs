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

/// Optional expandable body content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "lines", rename_all = "snake_case")]
pub enum ToolBody {
    #[default]
    None,
    Lines(Vec<String>),
    /// Compact diff body; lines keep leading `+`/`-` for semantic coloring.
    DiffLines(Vec<String>),
}

impl ToolBody {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::None => true,
            Self::Lines(lines) | Self::DiffLines(lines) => {
                lines.is_empty() || lines.iter().all(|line| line.trim().is_empty())
            }
        }
    }

    pub fn line_count(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Lines(lines) | Self::DiffLines(lines) => {
                lines.iter().map(|line| line.lines().count().max(1)).sum()
            }
        }
    }

    pub fn is_diff(&self) -> bool {
        matches!(self, Self::DiffLines(_))
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
    /// Show "ctrl+o to collapse" when expanded past budget / revealing hidden diff.
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
    /// - When collapsed: apply budget across facts first, then body.
    /// - Diff bodies ([`ToolBody::DiffLines`]) are HIDDEN when collapsed
    ///   (0 visible body lines) but still count toward `hidden_rows` and make
    ///   the card expandable if non-empty.
    /// - When expanded: show all facts and all body lines.
    /// - `expandable` is true if there is anything to reveal/hide via toggle:
    ///   non-empty collapsed diff, OR any hidden rows when collapsed, OR when
    ///   expanded after having been over budget / having a diff body that
    ///   collapses away.
    /// - When collapsed and `hidden_rows > 0`, caller shows
    ///   `... {hidden_rows} more lines, ctrl+o to expand`.
    /// - When expanded and `show_collapse_prompt`, show `ctrl+o to collapse`.
    ///
    /// Collapse prompt when expanded: true if a non-empty diff body exists OR
    /// total children (facts + body lines) > `max_lines`.
    pub fn display_plan(&self, max_lines: usize, expanded: bool) -> ToolCardDisplayPlan {
        let budget = max_lines.max(1);
        let fact_count = self.facts.len();
        let body_lines = self.body.line_count();
        let total_children = fact_count + body_lines;
        let non_empty_diff = self.body.is_diff() && body_lines > 0;

        if expanded {
            let show_collapse_prompt = non_empty_diff || total_children > budget;
            return ToolCardDisplayPlan {
                visible_facts: fact_count,
                visible_body_lines: body_lines,
                hidden_rows: 0,
                expandable: show_collapse_prompt,
                show_collapse_prompt,
            };
        }

        // Collapsed: budget covers facts first, then non-diff body lines.
        // Diff bodies stay fully hidden until expanded.
        let visible_facts = fact_count.min(budget);
        let remaining = budget.saturating_sub(visible_facts);
        let visible_body_lines = if self.body.is_diff() {
            0
        } else {
            body_lines.min(remaining)
        };
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
struct ParsedDiffFile {
    path: String,
    added: u64,
    removed: u64,
    compact_lines: Vec<String>,
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

/// Collapse a unified diff to compact add/remove/context lines for expand body.
pub fn compact_diff_lines(diff: &str, include_file_headers: bool) -> Vec<String> {
    let files = parse_unified_diff(diff);
    let mut lines = Vec::new();
    for file in files {
        if include_file_headers {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(file.path);
        }
        lines.extend(file.compact_lines);
    }
    lines
}

/// Parse a unified diff once into per-file sections used by stats and compact body.
fn parse_unified_diff(diff: &str) -> Vec<ParsedDiffFile> {
    let mut files = Vec::new();
    let mut current: Option<ParsedDiffFile> = None;
    let mut in_hunk = false;

    for line in diff.lines() {
        if should_exit_hunk(line) {
            in_hunk = false;
        }

        if let Some(path) = plus_file_path(line) {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(ParsedDiffFile {
                path,
                added: 0,
                removed: 0,
                compact_lines: Vec::new(),
            });
            continue;
        }

        if line.starts_with("@@") {
            in_hunk = true;
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
        match marker {
            b'+' => {
                file.added += 1;
                file.compact_lines.push(line.to_string());
            }
            b'-' => {
                file.removed += 1;
                file.compact_lines.push(line.to_string());
            }
            b' ' => {
                if let Some(content) = line.get(1..) {
                    file.compact_lines.push(content.to_string());
                }
            }
            _ => {}
        }
    }

    if let Some(file) = current {
        files.push(file);
    }
    files
}

fn plus_file_path(line: &str) -> Option<String> {
    if let Some(path) = line.strip_prefix("+++ b/") {
        return Some(path.to_string());
    }
    if line.starts_with("+++ /dev/null") {
        return Some("/dev/null".into());
    }
    None
}

/// Leave hunk mode at file boundaries so multi-file diffs need no blank separator.
fn should_exit_hunk(line: &str) -> bool {
    line.is_empty() || line.starts_with("diff --git") || is_file_header_line(line)
}

fn is_file_header_line(line: &str) -> bool {
    line.starts_with("--- a/")
        || line.starts_with("--- b/")
        || line.starts_with("--- /dev/null")
        || line.starts_with("+++ b/")
        || line.starts_with("+++ a/")
        || line.starts_with("+++ /dev/null")
}

#[cfg(test)]
#[path = "tool_card_tests.rs"]
mod tests;
