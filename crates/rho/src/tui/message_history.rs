use std::collections::VecDeque;

use {
    crate::app::interactive_presenter::InteractiveToolPresenter,
    rho_providers::model::{ContentBlock, Message, ToolCall},
};

use super::{Entry, ToolEntry, ToolEntryState};

pub(super) fn transcript_entries_from_messages(
    messages: &[Message],
    cwd: &std::path::Path,
) -> Vec<Entry> {
    let presenter = InteractiveToolPresenter::new(cwd.to_path_buf());
    let mut entries = Vec::new();
    let mut pending_tools = VecDeque::new();
    for message in messages {
        match message {
            Message::System(_) => {}
            Message::User(blocks) => {
                let text = super::render_message_blocks(blocks);
                if !text.is_empty() {
                    entries.push(Entry::User(text));
                }
            }
            Message::Assistant(blocks) => {
                let text = super::text_blocks(blocks);
                if !text.is_empty() {
                    entries.push(Entry::Assistant(text));
                }
                pending_tools.extend(blocks.iter().filter_map(|block| match block {
                    ContentBlock::ToolCall(call) => Some(call.clone()),
                    ContentBlock::Text(_) | ContentBlock::Image(_) => None,
                }));
            }
            Message::EnrichedAssistant(message) => {
                let blocks = &message.content;
                let text = super::text_blocks(blocks);
                if !text.is_empty() {
                    entries.push(Entry::Assistant(text));
                }
                pending_tools.extend(blocks.iter().filter_map(|block| match block {
                    ContentBlock::ToolCall(call) => Some(call.clone()),
                    ContentBlock::Text(_) | ContentBlock::Image(_) => None,
                }));
            }
            Message::AbortedAssistant(message) => {
                let text = super::text_blocks(&message.content);
                if !text.is_empty() {
                    entries.push(Entry::Assistant(text));
                }
                if let Some(tool_call) = message.tool_calls.last() {
                    let presented =
                        presenter.interrupted(tool_call.name.as_deref(), &tool_call.arguments);
                    entries.push(Entry::Tool(ToolEntry {
                        state: ToolEntryState::Finished {
                            ok: false,
                            display_style: presented.display_style,
                        },
                        display_lines: presented.display_lines,
                        expanded: false,
                        image: None,
                    }));
                }
                entries.push(Entry::Notice("model interrupted".into()));
            }
            Message::ToolResult(result) => {
                let call = pending_tools.pop_front().unwrap_or_else(|| ToolCall {
                    id: result.id.clone(),
                    name: "tool".into(),
                    arguments: serde_json::Value::Object(Default::default()),
                });
                let presented = presenter.historical(&call, result.ok, &result.content);
                entries.push(Entry::Tool(ToolEntry {
                    state: ToolEntryState::Finished {
                        ok: result.ok,
                        display_style: presented.display_style,
                    },
                    display_lines: presented.display_lines,
                    expanded: false,
                    image: None,
                }));
            }
        }
    }
    entries
}

#[cfg(test)]
#[path = "message_history_tests.rs"]
mod tests;
