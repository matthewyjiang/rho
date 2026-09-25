//! Text-summary compaction: the summarizer request and the replacement history
//! built from its response. [`CompactionPartition`] decides what stays
//! verbatim; this module only renders it for the model and assembles the
//! result.

use rho_providers::model::{ContentBlock, Message};
use rho_sdk::{model::SemanticMessage, CompactionTrigger, Error};
use rho_tools::tool::ToolResult;

use super::CompactionPartition;

/// Must keep the "Summarize the compacted conversation history" prefix: the
/// TUI fixture provider recognizes summary requests by it.
const SUMMARY_SYSTEM_PROMPT: &str = "\
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
/// Fails when the response has no summary text outside `<analysis>` blocks.
pub(crate) fn summary_replacement(
    partition: &CompactionPartition<'_>,
    trigger: CompactionTrigger,
    response: &[ContentBlock],
) -> Result<Vec<Message>, Error> {
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
    if let Some(summary) = message.as_compaction_summary() {
        return format!("earlier compaction summary:\n{}", summary.text());
    }
    match message.semantic() {
        SemanticMessage::System(text) => format!("system:\n{text}"),
        SemanticMessage::ToolImageSupplement(images) => {
            format!(
                "tool output images for {} ({}):\n{}",
                images.tool_name(),
                images.tool_call_id(),
                render_blocks(images.content())
            )
        }
        SemanticMessage::User(blocks) => format!("user:\n{}", render_blocks(blocks)),
        SemanticMessage::Assistant(blocks) => format!("assistant:\n{}", render_blocks(blocks)),
        SemanticMessage::EnrichedAssistant(message) => {
            let mut rendered = render_blocks(&message.content);
            if let Some(summary) = &message.reasoning_summary {
                rendered.push_str(&format!("\nreasoning summary:\n{summary}"));
            }
            format!("assistant:\n{rendered}")
        }
        SemanticMessage::AbortedAssistant(message) => {
            format!("assistant [aborted]:\n{}", render_blocks(&message.content))
        }
        SemanticMessage::ToolResult(result) => {
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
