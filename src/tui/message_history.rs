use std::collections::VecDeque;

use crate::{
    model::{ContentBlock, Message},
    tool::ToolDisplayStyle,
};

use super::{Entry, ToolEntry, ToolEntryState};

pub(super) fn transcript_entries_from_messages(messages: &[Message]) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut pending_tool_names = VecDeque::new();
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
                pending_tool_names.extend(blocks.iter().filter_map(|block| match block {
                    ContentBlock::ToolCall(call) => Some(call.name.clone()),
                    ContentBlock::Text(_) | ContentBlock::Image(_) => None,
                }));
            }
            Message::AbortedAssistant(message) => {
                let text = super::text_blocks(&message.content);
                if !text.is_empty() {
                    entries.push(Entry::Assistant(text));
                }
                if let Some(tool_call) = message.tool_calls.last() {
                    let mut display_lines =
                        vec![tool_call.name.clone().unwrap_or_else(|| "tool call".into())];
                    if !tool_call.arguments.is_empty() {
                        display_lines.push(tool_call.arguments.clone());
                    }
                    entries.push(Entry::Tool(ToolEntry {
                        state: ToolEntryState::Finished {
                            ok: false,
                            display_style: ToolDisplayStyle::default_tool(),
                        },
                        display_lines,
                        expanded: false,
                    }));
                }
                entries.push(Entry::Notice("model interrupted".into()));
            }
            Message::ToolResult(result) => {
                let name = pending_tool_names
                    .pop_front()
                    .unwrap_or_else(|| "tool".into());
                let display_style = ToolDisplayStyle::for_tool_name(&name);
                let mut display_lines = vec![name];
                if !result.content.trim().is_empty() {
                    display_lines.push(result.content.clone());
                }
                entries.push(Entry::Tool(ToolEntry {
                    state: ToolEntryState::Finished {
                        ok: result.ok,
                        display_style,
                    },
                    display_lines,
                    expanded: false,
                }));
            }
        }
    }
    entries
}
