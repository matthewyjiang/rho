//! Fixture providers read past the time lines Rho stamps on user and
//! assistant text ([`rho_sdk::model::MessageTime`]), as a real model would;
//! their scenarios match the words the user typed.

use rho_sdk::model::{ContentBlock, Message, MessageTime};

/// `messages` with each user and assistant entry's leading time line removed.
pub(crate) fn without_times(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            let content = match &mut message {
                Message::User(content) => Some(content),
                Message::EnrichedAssistant(assistant) => Some(&mut assistant.content),
                _ => None,
            };
            if let Some(text) = content.and_then(|content| {
                content.iter_mut().find_map(|block| match block {
                    ContentBlock::Text(text) => Some(text),
                    ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
                })
            }) {
                *text = without_time(text).to_owned();
            }
            message
        })
        .collect()
}

fn without_time(text: &str) -> &str {
    MessageTime::strip_rendered_prefix(text).unwrap_or(text)
}
