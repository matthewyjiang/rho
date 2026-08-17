use std::collections::VecDeque;

use {
    crate::app::interactive_presenter::InteractiveToolPresenter,
    rho_providers::model::{image_summary, ContentBlock, ImageContent, Message, ToolCall},
    rho_tools::tool_card::{ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus},
};

use super::{feed_image::FeedImage, ChatMedia, Entry, ToolEntry};

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

pub(super) fn generated_image_entry(
    preview: Result<Option<FeedImage>, String>,
    source: &ImageContent,
) -> Entry {
    let mut card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("image_generation", None),
    );
    let image = apply_generated_image_preview(&mut card, preview, source);
    card.push_fact(ToolFact::Meta {
        text: "finished".into(),
    });
    Entry::Tool(ToolEntry {
        card,
        expanded: false,
        image,
        started_at: None,
    })
}

fn apply_generated_image_preview(
    card: &mut ToolCard,
    preview: Result<Option<FeedImage>, String>,
    source: &ImageContent,
) -> Option<FeedImage> {
    match preview {
        Ok(None) => {
            card.push_fact(ToolFact::Text {
                text: image_summary(source),
            });
            None
        }
        Ok(image) => image,
        Err(error) => {
            card.push_fact(ToolFact::Error {
                text: format!("image preview unavailable: {error}"),
            });
            None
        }
    }
}

pub(super) fn render_user_entry(prompt: &str, media: &[ChatMedia]) -> String {
    let mut parts = Vec::new();
    if !prompt.is_empty() {
        parts.push(prompt.to_string());
    }
    parts.extend(
        media
            .iter()
            .enumerate()
            .map(|(index, media)| media.composer_label(index + 1)),
    );
    parts.join("\n")
}

pub(super) fn transcript_entries_from_messages(
    messages: &[Message],
    cwd: &std::path::Path,
    mut preview_image: impl FnMut(&ImageContent) -> Result<Option<FeedImage>, String>,
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
                push_generated_image_entries(&mut entries, blocks, &mut preview_image);
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
                push_generated_image_entries(&mut entries, blocks, &mut preview_image);
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
                push_generated_image_entries(&mut entries, &message.content, &mut preview_image);
                if let Some(tool_call) = message.tool_calls.last() {
                    let presented =
                        presenter.interrupted(tool_call.name.as_deref(), &tool_call.arguments);
                    entries.push(Entry::Tool(ToolEntry {
                        card: presented.card,
                        expanded: false,
                        image: None,
                        started_at: None,
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
                    card: presented.card,
                    expanded: false,
                    image: None,
                    started_at: None,
                }));
            }
        }
    }
    entries
}

fn push_generated_image_entries(
    entries: &mut Vec<Entry>,
    blocks: &[ContentBlock],
    preview_image: &mut impl FnMut(&ImageContent) -> Result<Option<FeedImage>, String>,
) {
    for block in blocks {
        if let ContentBlock::Image(image) = block {
            entries.push(generated_image_entry(preview_image(image), image));
        }
    }
}

impl super::App {
    pub(super) fn transcript_entries(&self, messages: &[Message]) -> Vec<Entry> {
        transcript_entries_from_messages(messages, &self.info.runtime.cwd, |_| Ok(None))
    }

    pub(super) fn set_history_entries(&mut self, entries: Vec<Entry>) {
        self.history.set_entries(entries);
        self.history.images_mut().clear();
    }
}
