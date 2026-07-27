//! Structured tool transcript cards for Call + Children rendering.
//!
//! Presenters build [`ToolCard`] values. Renderers draw them with multi-span
//! styles. Plain-text fallbacks keep older attach readers working for one
//! release while cards ship alongside `display_lines`.

use serde::{Deserialize, Serialize};

use crate::tool::ToolDisplayStyle;

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

impl ToolFamily {
    pub fn from_display_style(style: ToolDisplayStyle) -> Self {
        match style {
            ToolDisplayStyle::FileOrCommand => Self::FileCommand,
            ToolDisplayStyle::FileDiff => Self::FileDiff,
            ToolDisplayStyle::Web => Self::Web,
            ToolDisplayStyle::Skill => Self::Skill,
            ToolDisplayStyle::Questionnaire => Self::Form,
            ToolDisplayStyle::DefaultTool => Self::Default,
        }
    }

    pub fn display_style(self) -> ToolDisplayStyle {
        match self {
            Self::FileCommand => ToolDisplayStyle::FileOrCommand,
            Self::FileDiff => ToolDisplayStyle::FileDiff,
            Self::Web => ToolDisplayStyle::Web,
            Self::Skill => ToolDisplayStyle::Skill,
            Self::Form => ToolDisplayStyle::Questionnaire,
            Self::Agent | Self::Default => ToolDisplayStyle::DefaultTool,
        }
    }
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

    pub fn lines(&self) -> &[String] {
        match self {
            Self::None => &[],
            Self::Lines(lines) | Self::DiffLines(lines) => lines,
        }
    }

    pub fn is_diff(&self) -> bool {
        matches!(self, Self::DiffLines(_))
    }
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

    /// Body-only line budget used by expand/collapse chrome.
    pub fn expandable_line_count(&self) -> usize {
        self.body.line_count()
    }

    /// Plain-text fallback for attach readers that do not understand cards.
    pub fn to_display_lines(&self) -> Vec<String> {
        let mut lines = vec![self.header_text()];
        let fact_count = self.facts.len();
        for (index, fact) in self.facts.iter().enumerate() {
            let branch = if index + 1 == fact_count && self.body.is_empty() {
                "└"
            } else {
                "├"
            };
            lines.push(format!("  {branch} {}", fact_plain_text(fact)));
        }
        match &self.body {
            ToolBody::None => {}
            ToolBody::Lines(body) | ToolBody::DiffLines(body) => {
                for (index, line) in body.iter().enumerate() {
                    if index == 0 && self.facts.is_empty() {
                        // Keep body readable under the header without a lone branch.
                        if body.len() == 1 && !line.contains('\n') {
                            lines.push(format!("  └ {line}"));
                        } else {
                            lines.push(String::new());
                            lines.push(line.clone());
                        }
                    } else if index == 0 {
                        lines.push(String::new());
                        lines.push(line.clone());
                    } else {
                        lines.push(line.clone());
                    }
                }
            }
        }
        lines
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

    /// Build a card from plain transcript lines.
    ///
    /// Used when an older attach journal (or a non-presenter producer) only has
    /// line text. The first line becomes the call header; remaining lines become
    /// body text after tree-prefix stripping.
    pub fn from_plain_lines(status: ToolStatus, family: ToolFamily, lines: &[String]) -> Self {
        let mut lines = lines.iter().map(String::as_str);
        let heading = lines.next().unwrap_or("tool");
        let heading = strip_leading_status_marker(heading);
        let mut card = Self::new(status, family, ToolHeader::call(heading, None));
        let body = lines
            .map(|line| {
                line.strip_prefix("  ├ ")
                    .or_else(|| line.strip_prefix("  └ "))
                    .unwrap_or(line)
                    .to_string()
            })
            .collect::<Vec<_>>();
        let body = match body.as_slice() {
            [first, rest @ ..] if first.is_empty() => rest.to_vec(),
            _ => body,
        };
        if !body.is_empty() {
            card.body = ToolBody::Lines(body);
        }
        card
    }
}

fn strip_leading_status_marker(heading: &str) -> &str {
    let trimmed = heading.trim_start();
    for marker in ["● ", "✓ ", "✗ ", "■ ", "! ", "○ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    // Markers without a trailing space (compact legacy lines).
    for marker in ['●', '✓', '✗', '■', '!', '○'] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    trimmed
}

fn fact_plain_text(fact: &ToolFact) -> String {
    match fact {
        ToolFact::DiffStat {
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
        ToolFact::Exit { code, duration_ms } => match duration_ms {
            Some(ms) => {
                let secs = *ms as f64 / 1000.0;
                format!("exit {code} · {secs:.1}s")
            }
            None => format!("exit {code}"),
        },
        ToolFact::Count {
            label,
            value,
            detail,
        } => match detail {
            Some(detail) if !detail.is_empty() => format!("{value} {label} {detail}"),
            Some(_) | None => format!("{value} {label}"),
        },
        ToolFact::Meta { text } | ToolFact::Error { text } | ToolFact::Text { text } => {
            text.clone()
        }
        ToolFact::Progress { completed, total } => match total {
            Some(total) => format!("{completed}/{total}"),
            None => format!("{completed}"),
        },
    }
}

/// Per-file addition/removal counts extracted from a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFileStat {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// Extract per-file `+N -M` stats from a unified diff.
pub fn diff_file_stats(diff: &str) -> Vec<DiffFileStat> {
    let mut stats = Vec::new();
    let mut current: Option<DiffFileStat> = None;

    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            if let Some(stat) = current.take() {
                stats.push(stat);
            }
            current = Some(DiffFileStat {
                path: path.to_string(),
                added: 0,
                removed: 0,
            });
            continue;
        }
        if line.starts_with("+++ /dev/null") {
            if let Some(stat) = current.take() {
                stats.push(stat);
            }
            current = Some(DiffFileStat {
                path: "/dev/null".into(),
                added: 0,
                removed: 0,
            });
            continue;
        }
        let Some(stat) = current.as_mut() else {
            continue;
        };
        if line.starts_with("@@") || line.starts_with('\\') || line.is_empty() {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') if !line.starts_with("+++") => stat.added += 1,
            Some(b'-') if !line.starts_with("---") => stat.removed += 1,
            Some(_) | None => {}
        }
    }
    if let Some(stat) = current {
        stats.push(stat);
    }
    stats
}

/// Collapse a unified diff to compact add/remove/context lines for expand body.
pub fn compact_diff_lines(diff: &str, include_file_headers: bool) -> Vec<String> {
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
    lines
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn diff_stats_count_per_file() {
        let diff = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new

--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-before
+after
";
        assert_eq!(
            diff_file_stats(diff),
            vec![
                DiffFileStat {
                    path: "src/lib.rs".into(),
                    added: 1,
                    removed: 1,
                },
                DiffFileStat {
                    path: "src/main.rs".into(),
                    added: 1,
                    removed: 1,
                },
            ]
        );
    }

    #[test]
    fn card_display_lines_include_marker_and_tree() {
        let card = ToolCard::new(
            ToolStatus::Ok,
            ToolFamily::FileCommand,
            ToolHeader::call("edit_file", Some("theme.rs".into())),
        )
        .with_facts(vec![ToolFact::DiffStat {
            added: 54,
            removed: 2,
            path: Some("theme.rs".into()),
        }]);
        assert_eq!(
            card.to_display_lines(),
            vec![
                "✓ edit_file(theme.rs)".to_string(),
                "  └ +54 -2 lines | theme.rs".to_string(),
            ]
        );
    }

    #[test]
    fn tool_body_variants_round_trip() {
        for body in [
            ToolBody::None,
            ToolBody::Lines(vec!["line".into()]),
            ToolBody::DiffLines(vec!["+line".into()]),
        ] {
            let encoded = serde_json::to_string(&body).unwrap();
            assert_eq!(serde_json::from_str::<ToolBody>(&encoded).unwrap(), body);
        }
    }

    #[test]
    fn card_round_trips_through_json() {
        let card = ToolCard::new(
            ToolStatus::Running,
            ToolFamily::Web,
            ToolHeader::call("web_search", Some("\"rust\"".into())),
        )
        .with_facts(vec![ToolFact::Count {
            label: "results".into(),
            value: 8,
            detail: Some("stored".into()),
        }])
        .with_body(ToolBody::Lines(vec!["body".into()]));
        let encoded = serde_json::to_string(&card).unwrap();
        let decoded: ToolCard = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, card);
    }
}
