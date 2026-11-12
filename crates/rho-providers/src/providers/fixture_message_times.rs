//! Fixture providers read past the stamp line Rho puts on user messages
//! ([`rho_sdk::model::MessageTime`]), as a real model would; their scenarios
//! match the words the user typed.

use rho_sdk::model::{ContentBlock, Message, MessageTime};

/// `messages` with each user message's leading stamp line removed.
pub(crate) fn without_times(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            if let Message::User(content) = &mut message {
                if let Some(text) = content.iter_mut().find_map(|block| match block {
                    ContentBlock::Text(text) => Some(text),
                    ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
                }) {
                    if let Some(rest) = MessageTime::strip_rendered_prefix(text) {
                        *text = rest.to_owned();
                    }
                }
            }
            message
        })
        .collect()
}
