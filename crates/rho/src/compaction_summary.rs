//! Text-summary compaction: the summarizer request and the replacement history
//! built from its response. [`CompactionPartition`] decides what stays
//! verbatim; this module only renders it for the model and assembles the
//! result.

use rho_providers::model::{ContentBlock, Message};
use rho_sdk::{CompactionTrigger, Error};
use rho_tools::tool::ToolResult;

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

const SESSION_NO_TOOLS_INSTRUCTION: &str = "\
Stop working on the task. Do not call tools. Summarize the whole conversation \
above and reply with the summary only. The latest turns also stay verbatim \
after the summary, so keep their detail brief.";

const SESSION_ORIGINAL_REQUEST_INSTRUCTION: &str = "\
The user's first message above stays verbatim in context after compaction. \
Summarize only what the later turns add.";

const SESSION_PREVIOUS_SUMMARY_INSTRUCTION: &str = "\
An earlier compaction summary appears above. Update it with the turns after it \
and return one complete summary in the same sections. Keep details that still \
matter, revise ones the newer turns changed, and drop ones that no longer \
matter.";

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
    let mut sections = vec![SUMMARY_SYSTEM_PROMPT, SESSION_NO_TOOLS_INSTRUCTION];
    if !partition.first_turn().is_empty() {
        sections.push(SESSION_ORIGINAL_REQUEST_INSTRUCTION);
    }
    if partition.previous_summary().is_some() {
        sections.push(SESSION_PREVIOUS_SUMMARY_INSTRUCTION);
    }
    let mut messages = history.to_vec();
    messages.push(Message::user_text(sections.join("\n\n")));
    messages
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
    messages
        .into_iter()
        .map(render_message_for_summary)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_message_for_summary(message: &Message) -> String {
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
        HistoryMessage::ToolResult(result) => {
            format!("tool result:\n{}", render_tool_result(result))
        }
    }
}

fn render_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.clone(),
            ContentBlock::Image(image) => format!("[image: {}]", image.mime_type),
            ContentBlock::ToolCall(call) => serde_json::to_string(call).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_tool_result(result: &ToolResult) -> String {
    serde_json::to_string(result).unwrap_or_else(|_| result.content.clone())
}

#[cfg(test)]
#[path = "compaction_summary_tests.rs"]
mod tests;
