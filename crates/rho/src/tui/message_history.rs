use std::collections::VecDeque;

use {
    crate::app::interactive_presenter::InteractiveToolPresenter,
    rho_providers::model::{image_summary, ContentBlock, ImageContent, Message, ToolCall},
};

use super::{render::entry_lines, tool_output_ui::is_tool_entry, Entry, ToolEntry, ToolEntryState};

pub(super) fn recovered_history_tail(
    entries: &[Entry],
    width: usize,
    line_limit: usize,
    max_tool_output_lines: usize,
) -> (usize, Vec<Entry>) {
    let mut selected_start = entries.len();
    let mut line_count = 0usize;
    let mut next_is_tool = false;

    for (index, entry) in entries.iter().enumerate().rev() {
        let spacing = is_tool_entry(entry) && next_is_tool;
        let entry_line_count =
            entry_lines(entry, width, max_tool_output_lines).len() + usize::from(spacing);
        if selected_start < entries.len() && line_count + entry_line_count > line_limit {
            break;
        }
        selected_start = index;
        line_count += entry_line_count;
        next_is_tool = is_tool_entry(entry);
    }

    (selected_start, entries[selected_start..].to_vec())
}

pub(super) fn text_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn render_message_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            ContentBlock::Image(image) => Some(format!("[image: {}]", image_summary(image))),
            ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn render_user_entry(prompt: &str, images: &[ImageContent]) -> String {
    let mut parts = Vec::new();
    if !prompt.is_empty() {
        parts.push(prompt.to_string());
    }
    parts.extend(
        images
            .iter()
            .enumerate()
            .map(|(index, image)| format!("[image {}: {}]", index + 1, image_summary(image))),
    );
    parts.join("\n")
}

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
                let text = render_message_blocks(blocks);
                if !text.is_empty() {
                    entries.push(Entry::User(text));
                }
            }
            Message::Assistant(blocks) => {
                let text = text_blocks(blocks);
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
                let text = text_blocks(blocks);
                if !text.is_empty() {
                    entries.push(Entry::Assistant(text));
                }
                pending_tools.extend(blocks.iter().filter_map(|block| match block {
                    ContentBlock::ToolCall(call) => Some(call.clone()),
                    ContentBlock::Text(_) | ContentBlock::Image(_) => None,
                }));
            }
            Message::AbortedAssistant(message) => {
                let text = text_blocks(&message.content);
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
