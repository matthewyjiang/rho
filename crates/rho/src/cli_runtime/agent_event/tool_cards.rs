//! Generic ACP cards: bounded snapshots, without agent-specific raw payload policy.

use std::path::Path;

use agent_client_protocol::schema::v1::{
    ContentBlock, Diff, EmbeddedResourceResource, ToolCall, ToolCallContent, ToolCallLocation,
    ToolCallStatus, ToolCallUpdate, ToolKind,
};
use rho_tools::{
    tool::compact_display_path,
    tool_card::{
        compact_diff_rows, DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolFamily,
        ToolHeader, ToolStatus,
    },
};
use similar::{ChangeTag, TextDiff};

use crate::cli_runtime::{
    stream_effect::MAX_TOOL_PAYLOAD_CHARS,
    stream_format::{
        bound_text, set_lines_body, truncate, truncate_payload_lines, MAX_TOOL_BODY_LINES,
    },
};

// Header/error receipts carried over from the deleted Cursor `-p` cards: headers
// fit an 80-column pane; one-line errors use the Claude card summary width.
const HEADER_PRIMARY_CHARS: usize = 80;
const ERROR_SUMMARY_CHARS: usize = 160;

/// Minimal identity and bounded presentation retained between partial updates.
/// Raw input/output and full file contents are deliberately not retained.
#[derive(Debug)]
pub(super) struct StartedTool {
    title: String,
    kind: ToolKind,
    primary: Option<String>,
    pub(super) card: ToolCard,
}

impl StartedTool {
    pub(super) fn unknown() -> Self {
        Self {
            title: "Tool".into(),
            kind: ToolKind::Other,
            primary: None,
            card: ToolCard::new(
                ToolStatus::Running,
                ToolFamily::Default,
                ToolHeader::call("Tool", /*primary*/ None),
            ),
        }
    }

    fn refresh_header(&mut self, notices: &mut Vec<String>) {
        let (family, verb) = match self.kind {
            ToolKind::Read => (ToolFamily::FileCommand, "Read"),
            ToolKind::Search => (ToolFamily::FileCommand, "Search"),
            ToolKind::Execute => (ToolFamily::FileCommand, "Execute"),
            ToolKind::Edit => (ToolFamily::FileDiff, "Edit"),
            ToolKind::Delete => (ToolFamily::FileDiff, "Delete"),
            ToolKind::Move => (ToolFamily::FileDiff, "Move"),
            ToolKind::Fetch => (ToolFamily::Web, "Fetch"),
            ToolKind::Think => (ToolFamily::Default, "Think"),
            ToolKind::SwitchMode => (ToolFamily::Default, "SwitchMode"),
            ToolKind::Other => (ToolFamily::Default, "Tool"),
            _ => {
                notices.push("acp: unhandled tool kind".into());
                (ToolFamily::Default, "Tool")
            }
        };
        self.card.family = family;
        // Other tools have no generic verb: their bounded title is the identity.
        self.card.header = if self.kind == ToolKind::Other {
            ToolHeader::call(&self.title, self.primary.clone())
        } else {
            ToolHeader::call(
                verb,
                self.primary.clone().or_else(|| Some(self.title.clone())),
            )
        };
    }
}

/// Start a bounded card snapshot from the common ACP tool fields.
pub(super) fn started_card(call: &ToolCall, cwd: &Path) -> (StartedTool, Vec<String>) {
    let mut tool = StartedTool::unknown();
    tool.title = truncate(&call.title, HEADER_PRIMARY_CHARS);
    tool.kind = call.kind;
    tool.primary = location_primary(&call.locations, cwd);
    let mut notices = Vec::new();
    tool.card.status = card_status(call.status, &mut notices);
    tool.refresh_header(&mut notices);
    populate_content(&mut tool.card, &call.content, cwd, &mut notices);
    add_error_summary(&mut tool.card);
    (tool, notices)
}

/// Apply a partial snapshot; omitted fields retain the prior presentation.
/// Also used for in-progress updates, whose card remains running.
pub(super) fn finished_card(
    tool: &mut StartedTool,
    update: &ToolCallUpdate,
    cwd: &Path,
) -> (ToolCard, Vec<String>) {
    let fields = &update.fields;
    let mut notices = Vec::new();
    if let Some(title) = &fields.title {
        tool.title = truncate(title, HEADER_PRIMARY_CHARS);
    }
    if let Some(kind) = fields.kind {
        tool.kind = kind;
    }
    if let Some(locations) = &fields.locations {
        tool.primary = location_primary(locations, cwd);
    }
    if let Some(status) = fields.status {
        tool.card.status = card_status(status, &mut notices);
    }
    tool.refresh_header(&mut notices);
    if let Some(content) = &fields.content {
        // ACP collections replace, rather than append to, the prior snapshot.
        tool.card.facts.clear();
        tool.card.body = ToolBody::None;
        populate_content(&mut tool.card, content, cwd, &mut notices);
    }
    tool.card
        .facts
        .retain(|fact| !matches!(fact, ToolFact::Error { .. }));
    add_error_summary(&mut tool.card);
    (tool.card.clone(), notices)
}

fn card_status(status: ToolCallStatus, notices: &mut Vec<String>) -> ToolStatus {
    match status {
        ToolCallStatus::Pending | ToolCallStatus::InProgress => ToolStatus::Running,
        ToolCallStatus::Completed => ToolStatus::Ok,
        ToolCallStatus::Failed => ToolStatus::Error,
        _ => {
            notices.push("acp: unhandled tool status".into());
            ToolStatus::Running
        }
    }
}

fn location_primary(locations: &[ToolCallLocation], cwd: &Path) -> Option<String> {
    locations.first().map(|location| {
        truncate(
            &compact_display_path(cwd, &location.path.to_string_lossy()),
            HEADER_PRIMARY_CHARS,
        )
    })
}

/// Non-text blocks surface a placeholder, never opaque or base64 payloads.
pub(super) fn content_notice(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(_) => None,
        ContentBlock::Image(_) => Some("[image]".into()),
        ContentBlock::Audio(_) => Some("[audio]".into()),
        ContentBlock::ResourceLink(resource) => Some(format!(
            "[resource {}]",
            truncate(&resource.uri, HEADER_PRIMARY_CHARS)
        )),
        ContentBlock::Resource(resource) => Some(match &resource.resource {
            EmbeddedResourceResource::TextResourceContents(resource) => format!(
                "[resource {}]",
                truncate(&resource.uri, HEADER_PRIMARY_CHARS)
            ),
            EmbeddedResourceResource::BlobResourceContents(resource) => format!(
                "[resource {}]",
                truncate(&resource.uri, HEADER_PRIMARY_CHARS)
            ),
            _ => "acp: unhandled embedded resource".into(),
        }),
        _ => Some("acp: unhandled content block".into()),
    }
}

fn populate_content(
    card: &mut ToolCard,
    content: &[ToolCallContent],
    cwd: &Path,
    notices: &mut Vec<String>,
) {
    let mut text = String::new();
    let mut rows = Vec::new();
    // Reuse the expanded-card row receipt for content facts as well, so a
    // terminal/diff-only payload cannot grow an unbounded retained snapshot.
    for item in content.iter().take(MAX_TOOL_BODY_LINES) {
        match item {
            ToolCallContent::Content(content) => match &content.content {
                ContentBlock::Text(chunk) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&bound_text(
                        &chunk.text,
                        MAX_TOOL_PAYLOAD_CHARS,
                        "tool payload",
                    ));
                    text = bound_text(&text, MAX_TOOL_PAYLOAD_CHARS, "tool payload");
                }
                other => {
                    if let Some(message) = content_notice(other) {
                        notices.push(message);
                    }
                }
            },
            ToolCallContent::Diff(diff) => {
                let (diff_rows, added, removed) = diff_body(diff);
                rows.extend(diff_rows);
                card.push_fact(ToolFact::DiffStat {
                    added,
                    removed,
                    path: Some(truncate(
                        &compact_display_path(cwd, &diff.path.to_string_lossy()),
                        HEADER_PRIMARY_CHARS,
                    )),
                });
            }
            ToolCallContent::Terminal(terminal) => card.push_fact(ToolFact::Meta {
                text: format!(
                    "terminal: {}",
                    truncate(&terminal.terminal_id.to_string(), HEADER_PRIMARY_CHARS)
                ),
            }),
            _ => notices.push("acp: unhandled tool content".into()),
        }
    }
    if content.len() > MAX_TOOL_BODY_LINES {
        notices.push(format!(
            "acp: tool content budget {MAX_TOOL_BODY_LINES} blocks, received {}; remaining blocks omitted",
            content.len()
        ));
    }
    set_lines_body(card, &text);
    if !rows.is_empty() {
        // ToolBody has one body slot. Keep text alongside diffs as annotation
        // rows rather than silently dropping a tool's result summary.
        rows.extend(
            truncate_payload_lines(&text, MAX_TOOL_BODY_LINES)
                .into_iter()
                .map(|line| DiffRow::new(DiffRowKind::Meta, /*line*/ None, line)),
        );
        card.body = ToolBody::Diff(bound_diff_rows(rows));
    }
}

fn diff_body(diff: &Diff) -> (Vec<DiffRow>, u64, u64) {
    let changes = TextDiff::from_lines(
        diff.old_text.as_deref().unwrap_or_default(),
        diff.new_text.as_str(),
    );
    let mut added = 0;
    let mut removed = 0;
    for change in changes.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    // Count before truncating: the stat describes the real edit, not just the
    // rows that fit the attachment. The shared parser supplies line gutters.
    let unified = changes.unified_diff().header("file", "file").to_string();
    let bounded = bound_text(&unified, MAX_TOOL_PAYLOAD_CHARS, "diff");
    let mut rows = compact_diff_rows(&bounded, /*include_file_headers*/ false);
    if bounded != unified {
        rows.push(DiffRow::new(
            DiffRowKind::Skip,
            /*line*/ None,
            "… [truncated diff]",
        ));
    }
    (rows, added, removed)
}

/// Bound all diffs together, including a single very wide line, visibly.
fn bound_diff_rows(rows: Vec<DiffRow>) -> Vec<DiffRow> {
    let total = rows.len();
    let mut kept = Vec::new();
    let mut remaining = MAX_TOOL_PAYLOAD_CHARS;
    let mut truncated = false;
    for mut row in rows.into_iter().take(MAX_TOOL_BODY_LINES) {
        let chars = row.text.chars().count();
        if chars > remaining {
            row.text = truncate(&row.text, remaining);
            kept.push(row);
            truncated = true;
            break;
        }
        remaining -= chars;
        kept.push(row);
    }
    if kept.len() < total || truncated {
        kept.push(DiffRow::new(
            DiffRowKind::Skip,
            /*line*/ None,
            "… [truncated diff]",
        ));
    }
    kept
}

fn add_error_summary(card: &mut ToolCard) {
    if card.status != ToolStatus::Error {
        return;
    }
    let summary = match &card.body {
        ToolBody::Lines(lines) => lines.first().map(String::as_str),
        ToolBody::Diff(_) | ToolBody::None => None,
    }
    .unwrap_or("tool failed");
    let text = truncate(summary, ERROR_SUMMARY_CHARS);
    card.push_fact(ToolFact::Error { text });
}

#[cfg(test)]
#[path = "tool_cards_tests.rs"]
mod tests;
