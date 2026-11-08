//! Tool cards in Rho's Call + Children grammar: a header, facts under it,
//! and an expandable body.
//!
//! Presenters build [`ToolCard`] values; renderers draw them with styles
//! from the card's structure, never by parsing its text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tool-family identity, which picks the header verb's color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolFamily {
    FileCommand,
    FileDiff,
    Web,
    Skill,
    Form,
    Agent,
    Default,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Lifecycle status for the card's marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolCardStatus {
    Running,
    Ok,
    Error,
    Interrupted,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl ToolCardStatus {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Running | Self::Unknown => "●",
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
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
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it. Renderers show the status marker alone.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
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
    /// Shell timeout budget in seconds; `None` when the call has no timeout.
    /// Structured so hosts can decorate it live (elapsed clock) without
    /// parsing display text.
    Timeout {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seconds: Option<u64>,
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
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it. Its plain text is empty.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl ToolFact {
    /// Plain text for a fact, used by text-only surfaces and tests.
    pub fn plain_text(&self) -> String {
        match self {
            Self::DiffStat {
                added,
                removed,
                path,
            } => format_diff_stat_plain(*added, *removed, path.as_deref()),
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
            Self::Timeout { seconds } => match seconds {
                Some(seconds) => format!("timeout {seconds}s"),
                None => "timeout none".into(),
            },
            Self::Progress { completed, total } => match total {
                Some(total) => format!("{completed}/{total}"),
                None => format!("{completed}"),
            },
            Self::Unknown => String::new(),
        }
    }
}

/// What a diff row represents, which decides its sign and color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it. Renderers treat it as [`Self::Meta`].
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl DiffRowKind {
    /// Sign column text, so a color-stripped card still reads correctly.
    pub fn sign(self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Removed => "-",
            Self::Context | Self::File | Self::Skip | Self::Meta | Self::Unknown => " ",
        }
    }
}

/// One row of a compact diff body, with the line numbers for its gutter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    /// Line number: the new file's for context and additions, the old file's
    /// for removals. Absent for headings and for patch text without hunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub text: String,
    /// Per-file `+N -M` counts on multi-file [`DiffRowKind::File`] section
    /// headers. Structured so hosts style counts without parsing display text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<(u64, u64)>,
}

impl DiffRow {
    pub fn new(kind: DiffRowKind, line: Option<u32>, text: impl Into<String>) -> Self {
        Self {
            kind,
            line,
            text: text.into(),
            stats: None,
        }
    }

    /// Multi-file section header: path plus optional content stats.
    pub fn file_header(path: impl Into<String>, stats: Option<(u64, u64)>) -> Self {
        Self {
            kind: DiffRowKind::File,
            line: None,
            text: path.into(),
            stats,
        }
    }

    /// Gutter number, sign and text as one string, for text-only surfaces.
    pub fn plain_text(&self) -> String {
        match self.kind {
            DiffRowKind::File => match self.stats {
                Some((added, removed)) => {
                    format_diff_stat_plain(added, removed, Some(self.text.as_str()))
                }
                None => self.text.clone(),
            },
            DiffRowKind::Skip | DiffRowKind::Meta | DiffRowKind::Unknown => self.text.clone(),
            DiffRowKind::Context | DiffRowKind::Added | DiffRowKind::Removed => match self.line {
                Some(line) => format!("{line} {}{}", self.kind.sign(), self.text),
                None => format!("{}{}", self.kind.sign(), self.text),
            },
        }
    }
}

/// Shared plain-text form for DiffStat facts and File section headers.
fn format_diff_stat_plain(added: u64, removed: u64, path: Option<&str>) -> String {
    let stats = format!("+{added} -{removed} lines");
    match path {
        Some(path) if !path.is_empty() => format!("{stats} | {path}"),
        Some(_) | None => stats,
    }
}

/// Which end of a [`ToolBody::Code`] stays visible when its card is
/// collapsed. The full body remains available on expansion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CodeWindow {
    /// Show the opening rows, for finished or replayed source.
    #[default]
    Head,
    /// Show the newest rows, for source the model is still generating.
    Tail,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it. Renderers treat it as [`Self::Head`].
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl CodeWindow {
    fn is_head(&self) -> bool {
        matches!(self, Self::Head)
    }
}

/// Optional expandable body content.
///
/// Every variant except [`Self::Diff`] carries plain text lines; [`Self::Code`]
/// and [`Self::Search`] add the metadata renderers need to highlight them.
///
/// Closed: its variants carry their content under `lines`, which serde
/// cannot read into a unit `Unknown`, so a new variant is a protocol major.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(tag = "kind", content = "lines", rename_all = "snake_case")]
pub enum ToolBody {
    #[default]
    None,
    Lines(Vec<String>),
    /// Source code highlighted as `language`, a syntax name or file
    /// extension such as `python` or `rs`.
    Code {
        language: String,
        #[serde(default, skip_serializing_if = "CodeWindow::is_head")]
        window: CodeWindow,
        lines: Vec<String>,
    },
    /// Search result previews with the query that produced them, so
    /// renderers can highlight matches with the search tool's semantics.
    Search {
        pattern: String,
        /// The pattern is a literal substring rather than a regex.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        literal: bool,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        case_sensitive: bool,
        lines: Vec<String>,
    },
    /// Compact diff body with per-row line numbers and change kinds.
    Diff(Vec<DiffRow>),
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl ToolBody {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::None => true,
            Self::Lines(lines) | Self::Code { lines, .. } | Self::Search { lines, .. } => {
                lines.is_empty() || lines.iter().all(|line| line.trim().is_empty())
            }
            Self::Diff(rows) => rows.is_empty(),
        }
    }

    pub fn is_diff(&self) -> bool {
        matches!(self, Self::Diff(_))
    }

    /// Body content as plain lines, for text-only surfaces and tests.
    pub fn plain_lines(&self) -> Vec<String> {
        match self {
            Self::None => Vec::new(),
            Self::Lines(lines) | Self::Code { lines, .. } | Self::Search { lines, .. } => {
                lines.clone()
            }
            Self::Diff(rows) => rows.iter().map(DiffRow::plain_text).collect(),
        }
    }
}

/// Structured tool presentation for Call + Children rendering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCard {
    pub status: ToolCardStatus,
    pub family: ToolFamily,
    pub header: ToolHeader,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<ToolFact>,
    #[serde(default, skip_serializing_if = "ToolBody::is_empty")]
    pub body: ToolBody,
}

impl ToolCard {
    pub fn new(status: ToolCardStatus, family: ToolFamily, header: ToolHeader) -> Self {
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

    /// Appends each non-blank line of `notices` as a [`ToolFact::Meta`].
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

    /// The header as one line, marker first, for text-only surfaces.
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
            ToolHeader::Unknown => marker.to_owned(),
        }
    }
}

#[cfg(test)]
#[path = "card_tests.rs"]
mod tests;
