//! Nested-call records rendered as the codemode tool's progress text for its
//! live TUI card, ACP, and the automation protocol.

use serde_json::Value;

/// Display width for one call's arguments, matching Pi's collapsed codemode
/// rows (`COLLAPSED_ARGS_CHARS = 80`). The full arguments stay in the script.
const ARGS_DISPLAY_CHARS: usize = 80;
/// Display width for a running call's latest progress line or failure reason.
const DETAIL_DISPLAY_CHARS: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NestedCallStatus {
    Running,
    Ok,
    Error,
    Cancelled,
}

/// One nested tool call made by a script, in start order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NestedCallRecord {
    pub name: String,
    /// Primary argument (or compact JSON), cut for display; empty for `{}`.
    pub args: String,
    pub status: NestedCallStatus,
    pub duration_ms: Option<u64>,
    /// Latest progress line while running, or the first line of a failure.
    pub detail: Option<String>,
}

impl NestedCallRecord {
    /// Rows lead with the argument that names the work (`command`, `path`,
    /// `query`, ...) as plain text, the same pick MCP cards promote. Calls
    /// without one fall back to compact JSON.
    pub(super) fn running(name: &str, arguments: &Value) -> Self {
        let args = match arguments {
            Value::Object(map) if map.is_empty() => String::new(),
            Value::Null => String::new(),
            other => crate::tools::mcp::display::primary_argument(other).map_or_else(
                || cut(&other.to_string(), ARGS_DISPLAY_CHARS),
                |(_, value)| cut(&value, ARGS_DISPLAY_CHARS),
            ),
        };
        Self {
            name: name.to_owned(),
            args,
            status: NestedCallStatus::Running,
            duration_ms: None,
            detail: None,
        }
    }

    /// Keeps the latest progress line: the last non-blank line of `text`.
    pub(super) fn set_progress(&mut self, text: &str) {
        self.detail = text
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .map(|line| cut(line, DETAIL_DISPLAY_CHARS));
    }

    /// Keeps the failure reason: the first non-blank line of `text`.
    pub(super) fn set_error(&mut self, text: &str) {
        self.detail = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| cut(line, DETAIL_DISPLAY_CHARS));
    }

    /// One display row: `✓ bash ls -la 12ms · detail`.
    pub(super) fn row(&self) -> String {
        let marker = match self.status {
            NestedCallStatus::Running => "●",
            NestedCallStatus::Ok => "✓",
            NestedCallStatus::Error => "✗",
            NestedCallStatus::Cancelled => "⊘",
        };
        let mut row = format!("{marker} {}", self.name);
        if !self.args.is_empty() {
            row.push(' ');
            row.push_str(&self.args);
        }
        if let Some(ms) = self.duration_ms {
            row.push(' ');
            row.push_str(&format_duration(ms));
        }
        if let Some(detail) = self.detail.as_deref().filter(|detail| !detail.is_empty()) {
            row.push_str(" · ");
            row.push_str(detail);
        }
        row
    }
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

fn cut(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
#[path = "call_log_tests.rs"]
mod tests;
