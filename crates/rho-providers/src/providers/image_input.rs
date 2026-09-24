//! Strip image input for models the catalog marks as text-only.
//!
//! Every built-in transport goes through [`crate::impl_sdk_model_provider`],
//! which calls [`gate_images`] before conversion. The wire converters then
//! never see images the model cannot read, and the model gets a text note
//! instead of a provider 400 or a silently ignored block.

use std::borrow::Cow;

use rho_sdk::model::{ContentBlock, Message, ModelIdentity};

use crate::model::models_dev::{self, ImageInput};

/// Model-visible replacement for a stripped image.
pub(crate) const IMAGE_UNSUPPORTED_TEXT: &str =
    "[image omitted: the current model does not accept image input]";

/// Returns history safe to send to `identity`.
///
/// Borrowed unchanged unless the history carries an image and the catalog
/// explicitly marks the model `Unsupported`. `Unknown` keeps images, so
/// uncatalogued hosts behave as before.
pub fn gate_images<'a>(identity: &ModelIdentity, messages: &'a [Message]) -> Cow<'a, [Message]> {
    if !messages.iter().any(user_has_image) {
        return Cow::Borrowed(messages);
    }
    match models_dev::image_input(&identity.provider, &identity.model) {
        ImageInput::Supported | ImageInput::Unknown => Cow::Borrowed(messages),
        ImageInput::Unsupported => Cow::Owned(strip_user_images(messages)),
    }
}

fn user_has_image(message: &Message) -> bool {
    matches!(message, Message::User(blocks) if blocks.iter().any(|block| matches!(block, ContentBlock::Image(_))))
}

/// Replaces user-role images with [`IMAGE_UNSUPPORTED_TEXT`].
///
/// Only user-role images are touched: assistant images already degrade to
/// text inside each protocol converter.
fn strip_user_images(messages: &[Message]) -> Vec<Message> {
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
            other => other.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "image_input_tests.rs"]
mod tests;
