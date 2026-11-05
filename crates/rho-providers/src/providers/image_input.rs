//! Strip image input, including tool-result images, for models the catalog
//! marks as text-only.
//!
//! Every built-in transport goes through [`crate::impl_sdk_model_provider`],
//! which calls [`gate_images`] before conversion. The wire converters then
//! never see images the model cannot read, and the model gets a text note
//! instead of a provider 400 or a silently ignored block.

use std::borrow::Cow;

use rho_sdk::model::{ContentBlock, Message, ModelIdentity, ToolResult};

use crate::model::models_dev::{self, ImageInput};

/// Model-visible replacement for a stripped image.
pub(crate) const IMAGE_UNSUPPORTED_TEXT: &str =
    "[image omitted: the current model does not accept image input]";

/// Returns history safe to send to `identity`.
///
/// Borrowed unchanged unless the history carries a user or tool-result image
/// and the catalog explicitly marks the model `Unsupported`. `Unknown` keeps
/// images, so uncatalogued hosts behave as before.
pub fn gate_images<'a>(identity: &ModelIdentity, messages: &'a [Message]) -> Cow<'a, [Message]> {
    if !messages.iter().any(has_input_image) {
        return Cow::Borrowed(messages);
    }
    // Stale rows count: modalities rarely change, and an offline session should
    // still avoid sending images to a text-only model.
    let image_input = models_dev::cached_model_metadata(&identity.provider, &identity.model)
        .map_or(ImageInput::Unknown, |metadata| metadata.image_input);
    match image_input {
        ImageInput::Supported | ImageInput::Unknown => Cow::Borrowed(messages),
        ImageInput::Unsupported => Cow::Owned(strip_input_images(messages)),
    }
}

fn has_input_image(message: &Message) -> bool {
    match message {
        Message::User(blocks) => blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Image(_))),
        Message::ToolResult(result) => !result.images.is_empty(),
        Message::System(_)
        | Message::Assistant(_)
        | Message::EnrichedAssistant(_)
        | Message::AbortedAssistant(_) => false,
    }
}

/// Replaces user and tool-result images with [`IMAGE_UNSUPPORTED_TEXT`].
///
/// Assistant images already degrade to text inside each protocol converter.
fn strip_input_images(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|message| match message {
            Message::User(blocks) => Message::User(
                blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Image(_) => ContentBlock::Text(IMAGE_UNSUPPORTED_TEXT.into()),
                        ContentBlock::Text(_) | ContentBlock::ToolCall(_) => block.clone(),
                    })
                    .collect(),
            ),
            Message::ToolResult(result) if !result.images.is_empty() => {
                let mut content = result.content.clone();
                for _ in &result.images {
                    content.push('\n');
                    content.push_str(IMAGE_UNSUPPORTED_TEXT);
                }
                Message::ToolResult(ToolResult::new(result.id.clone(), result.status, content))
            }
            other => other.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "image_input_tests.rs"]
mod tests;
