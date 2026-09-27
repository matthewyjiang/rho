//! Text-summary compaction: the summarizer request and the replacement history
//! built from its response. [`CompactionPartition`] decides what stays
//! verbatim; this module only renders it for the model and assembles the
//! result.

use std::collections::HashMap;

use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};
use rho_sdk::{CompactionTrigger, Error};

use super::CompactionPartition;
use crate::history_message::HistoryMessage;

/// Must keep the "Summarize the compacted conversation history" prefix: the
/// TUI fixture provider recognizes summary requests by it, as a system prompt
/// or as the trailing instruction of a session-history request.
pub(crate) const SUMMARY_SYSTEM_PROMPT: &str = "\
Summarize the compacted conversation history for continuation. The original \
transcript is stored separately; this summary replaces only older model context \
for an agent that will keep working on the task.

You may first think in an <analysis>...</analysis> block. It is removed before \
the summary is used.

Then write the summary with exactly these Markdown sections, in this order. \
Write \"None\" under a section with nothing to report.

## Original request
## Constraints and preferences
## Decisions and rationale
## Files touched and their current state
## Commands run and test results
## Errors and fixes
## Open tasks
## Exact next step

Keep exact paths, commands, identifiers, error text, and numbers. Be concise \
and factual. Do not invent progress that the transcript does not show.";

const PREVIOUS_SUMMARY_INSTRUCTION: &str = "\
An earlier compaction already summarized the conversation before these turns. \
Update that summary with the new turns and return one complete summary in the \
same sections. Keep details that still matter, revise ones the new turns \
changed, and drop ones that no longer matter.";

const ORIGINAL_REQUEST_INSTRUCTION: &str = "\
The user's original request stays verbatim in context after compaction. It is \
included for reference; summarize only what the turns below add.";

const SESSION_TASK_INSTRUCTION: &str = "\
Stop working on the task. Do not call tools. Reply with the summary only.

The summary replaces one span of the conversation above, named below. That \
span will be deleted, so keep its paths, commands, identifiers, errors, and \
decisions. Where this message names a span, summarize that span rather than \
the whole conversation. Messages that stay verbatim after compaction should \
stay brief.";

const SESSION_FIRST_TURN_INSTRUCTION: &str = "\
The user's first message stays verbatim. Do not summarize that message; \
summarize the span named below.";

const SESSION_PREVIOUS_SUMMARY_INSTRUCTION: &str = "\
An earlier compaction summary appears above. Update it from the span named \
below and return one complete summary in the same sections. Keep details that \
still matter, revise ones the newer turns changed, and drop ones that no \
longer matter.";

/// Summary request that resends `history`, the session's request history,
/// unchanged and appends the instruction as a final user message.
///
/// The provider cached that history for the session's last turn, so this
/// request is billed mostly as cache reads. The caller must send it with the
/// session model, reasoning level, tool specs, and prompt cache key; changing
/// any of them misses the cache. `partition` must split `history`.
pub(crate) fn build_session_summary_request(
    history: &[Message],
    partition: &CompactionPartition<'_>,
) -> Vec<Message> {
    let mut sections = vec![
        SUMMARY_SYSTEM_PROMPT.to_string(),
        SESSION_TASK_INSTRUCTION.to_string(),
    ];
    if !partition.first_turn().is_empty() {
        sections.push(SESSION_FIRST_TURN_INSTRUCTION.to_string());
    }
    if partition.previous_summary().is_some() {
        sections.push(SESSION_PREVIOUS_SUMMARY_INSTRUCTION.to_string());
    }
    sections.push(deleted_span_instruction(partition));
    let mut messages = history.to_vec();
    messages.push(Message::user_text(sections.join("\n\n")));
    messages
}

/// Names the messages [`CompactionPartition`] will delete, and the verbatim
/// tail those messages stop before. Quotes only a short marker so the suffix
/// stays small; the history itself is already in the cached prefix.
fn deleted_span_instruction(partition: &CompactionPartition<'_>) -> String {
    let kept_latest = partition.kept_latest_user();
    let start = partition
        .summarized()
        .find(|message| kept_latest.is_none_or(|kept| !std::ptr::eq(*message, kept)));
    let mut lines = Vec::new();
    if let Some(kept) = kept_latest {
        lines.push(format!(
            "This user message stays verbatim even though it sits among the \
             turns being summarized. Mention it only briefly:\n{}",
            message_marker(kept)
        ));
    }
    match (start, partition.recent_tail().first()) {
        (Some(start), Some(tail)) => lines.push(format!(
            "Summarize from this message, inclusive, up to but not including \
             the verbatim tail:\n\nDeleted span starts at:\n{}\n\nVerbatim tail \
             starts at:\n{}",
            message_marker(start),
            message_marker(tail)
        )),
        (Some(start), None) => lines.push(format!(
            "Summarize from this message through the end of the conversation:\n{}",
            message_marker(start)
        )),
        (None, _) => lines.push("Summarize the conversation above.".to_string()),
    }
    lines.join("\n\n")
}

/// Role plus the first content line, truncated. Enough to point at one
/// message in the history without copying it into the uncached suffix.
fn message_marker(message: &Message) -> String {
    let rendered = render_message_for_summary(message, &HashMap::new());
    let content = rendered
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.ends_with(':'))
        .unwrap_or("message");
    const MAX_CHARS: usize = 120;
    let content: String = content.chars().take(MAX_CHARS).collect();
    let role = match HistoryMessage::of(message) {
        HistoryMessage::CompactionSummary(_) => "earlier compaction summary",
        HistoryMessage::System(_) => "system",
        HistoryMessage::ToolImageSupplement(_) => "tool output images",
        HistoryMessage::User(_) => "user",
        HistoryMessage::Assistant(_)
        | HistoryMessage::EnrichedAssistant(_)
        | HistoryMessage::AbortedAssistant(_) => "assistant",
        HistoryMessage::ToolResult(_) => "tool result",
    };
    format!("{role}: {content}")
}

/// Summary request that renders the history as one transcript under its own
/// system prompt. Works on any model, but shares no cached prefix with the
/// session.
pub(crate) fn build_summary_request_messages(partition: &CompactionPartition<'_>) -> Vec<Message> {
    let mut sections = Vec::new();
    if !partition.first_turn().is_empty() {
        sections.push(format!(
            "{ORIGINAL_REQUEST_INSTRUCTION}\n\n<original-request>\n{}\n</original-request>",
            render_messages_for_summary(partition.first_turn())
        ));
    }
    if let Some(previous) = partition.previous_summary() {
        sections.push(format!(
            "{PREVIOUS_SUMMARY_INSTRUCTION}\n\n<previous-summary>\n{}\n</previous-summary>",
            previous.trim()
        ));
    }
    sections.push(format!(
        "<conversation>\n{}\n</conversation>",
        render_messages_for_summary(partition.summarized())
    ));
    vec![
        Message::System(SUMMARY_SYSTEM_PROMPT.into()),
        Message::user_text(sections.join("\n\n")),
    ]
}

/// Replacement history from the summarizer's response, labeled by `trigger`.
/// Fails when the response calls a tool, which is never executed, or has no
/// summary text outside `<analysis>` blocks.
pub(crate) fn summary_replacement(
    partition: &CompactionPartition<'_>,
    trigger: CompactionTrigger,
    response: &[ContentBlock],
) -> Result<Vec<Message>, Error> {
    if calls_tool(response) {
        return Err(Error::InvalidHostResponse {
            message: "compaction model called a tool instead of writing a summary".into(),
        });
    }
    let text = response
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<String>();
    let summary = strip_analysis(&text);
    if summary.is_empty() {
        return Err(Error::InvalidHostResponse {
            message: "compaction model returned no summary text".into(),
        });
    }
    Ok(partition.replacement(Message::compaction_summary(trigger, summary)))
}

/// Whether a summary response asked for a tool instead of answering.
pub(crate) fn calls_tool(response: &[ContentBlock]) -> bool {
    response
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
}

/// Removes `<analysis>` scratchpads. An unclosed block runs to the end.
fn strip_analysis(text: &str) -> String {
    const OPEN: &str = "<analysis>";
    const CLOSE: &str = "</analysis>";
    let mut kept = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        kept.push_str(&rest[..start]);
        match rest[start..].find(CLOSE) {
            Some(end) => rest = &rest[start + end + CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    kept.push_str(rest);
    kept.trim().to_owned()
}

fn render_messages_for_summary<'a>(messages: impl IntoIterator<Item = &'a Message>) -> String {
    let mut names = HashMap::new();
    let mut rendered = Vec::new();
    for message in messages {
        // Names seen so far only. A reused call id must not relabel an earlier
        // result, and a result whose call is outside this transcript stays
        // `unknown` instead of dropping the id and status.
        rendered.push(render_message_for_summary(message, &names));
        for block in message_content_blocks(message) {
            if let ContentBlock::ToolCall(call) = block {
                names.insert(call.id.as_str(), call.name.as_str());
            }
        }
    }
    rendered.join("\n\n")
}

fn message_content_blocks(message: &Message) -> &[ContentBlock] {
    match HistoryMessage::of(message) {
        HistoryMessage::User(blocks) | HistoryMessage::Assistant(blocks) => blocks,
        HistoryMessage::EnrichedAssistant(message) => message.content.as_slice(),
        HistoryMessage::AbortedAssistant(message) => message.content.as_slice(),
        HistoryMessage::ToolImageSupplement(images) => images.content(),
        HistoryMessage::CompactionSummary(_)
        | HistoryMessage::System(_)
        | HistoryMessage::ToolResult(_) => &[],
    }
}

fn render_message_for_summary(message: &Message, names: &HashMap<&str, &str>) -> String {
    match HistoryMessage::of(message) {
        HistoryMessage::CompactionSummary(summary) => {
            format!("earlier compaction summary:\n{}", summary.text())
        }
        HistoryMessage::System(text) => format!("system:\n{text}"),
        HistoryMessage::ToolImageSupplement(images) => {
            format!(
                "tool output images for {} ({}):\n{}",
                images.tool_name(),
                images.tool_call_id(),
                render_blocks(images.content())
            )
        }
        HistoryMessage::User(blocks) => format!("user:\n{}", render_blocks(blocks)),
        HistoryMessage::Assistant(blocks) => format!("assistant:\n{}", render_blocks(blocks)),
        HistoryMessage::EnrichedAssistant(message) => {
            let mut rendered = render_blocks(&message.content);
            if let Some(summary) = &message.reasoning_summary {
                rendered.push_str(&format!("\nreasoning summary:\n{summary}"));
            }
            format!("assistant:\n{rendered}")
        }
        HistoryMessage::AbortedAssistant(message) => {
            format!("assistant [aborted]:\n{}", render_blocks(&message.content))
        }
        HistoryMessage::ToolResult(result) => render_tool_result(result, names),
    }
}

fn render_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.clone(),
            ContentBlock::Image(image) => format!("[image: {}]", image.mime_type),
            ContentBlock::ToolCall(call) => render_tool_call(call),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_call(call: &ToolCall) -> String {
    let arguments = serde_json::to_string_pretty(&call.arguments)
        .unwrap_or_else(|_| call.arguments.to_string());
    format!("tool call {} ({}):\n{arguments}", call.name, call.id)
}

fn render_tool_result(result: &ToolResult, names: &HashMap<&str, &str>) -> String {
    let name = names.get(result.id.as_str()).copied().unwrap_or("unknown");
    let status = if result.ok { "ok" } else { "error" };
    format!(
        "tool result {name} ({}) [{status}]:\n{}",
        result.id, result.content
    )
}

#[cfg(test)]
#[path = "compaction_summary_tests.rs"]
mod tests;
