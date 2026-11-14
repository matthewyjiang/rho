//! A prompt's content blocks as the user input its turn runs: the one
//! conversion behind the daemon's `rho/session/prompt` and ACP's
//! `session/prompt` (#1670).
//!
//! Text and images stay typed, once each image's bytes check out.
//! Resources, resource links, and blocks rho cannot read become fenced
//! text, labelled with their kind and URI, so the model still sees them.
//! Context blocks, which the client adds and the person did not write, are
//! kept apart: only the model gets them, never the saved message.

use base64::Engine as _;
use rho_protocol::ContentBlock;
use rho_sdk::{
    model::{self, ImageContent},
    UserInput,
};
use rho_tools::MAX_IMAGE_FILE_BYTES;

/// Why a prompt's content cannot start a turn. The client sent it, so
/// callers answer it as invalid params. `index` is the block's position in
/// the prompt, from 0.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum InvalidPrompt {
    #[error("a prompt needs at least one content block besides context")]
    Empty,
    #[error("content block {index}: image data is not valid base64")]
    ImageNotBase64 { index: usize },
    /// The limit pasted and previewed images have (`MAX_IMAGE_FILE_BYTES`).
    #[error(
        "content block {index}: image is {bytes} bytes, over the {MAX_IMAGE_FILE_BYTES} byte image limit"
    )]
    ImageTooLarge { index: usize, bytes: usize },
    #[error("content block {index}: image is not a PNG, JPEG, GIF, or WebP")]
    ImageUnrecognized { index: usize },
    #[error("content block {index}: image is declared {declared} but its data is {actual}")]
    ImageMismatch {
        index: usize,
        declared: String,
        actual: &'static str,
    },
}

/// A prompt's content as its turn runs it: the person's message, and the
/// context the client added for the model alone.
#[derive(Debug, PartialEq)]
pub(crate) struct Prompt {
    /// What the person sent: the message the session saves and shows.
    pub(crate) person: UserInput,
    /// The prompt's context blocks, in order.
    context: Vec<String>,
}

impl Prompt {
    /// `model`, the input a turn of this prompt sends, after the prompt's
    /// context.
    pub(crate) fn for_model(&self, model: UserInput) -> UserInput {
        if self.context.is_empty() {
            return model;
        }
        let blocks = self
            .context
            .iter()
            .cloned()
            .map(model::ContentBlock::Text)
            .chain(model.blocks().iter().cloned())
            .collect();
        UserInput::content(blocks).expect("context blocks keep the input non-empty")
    }
}

/// The prompt `blocks` make. Refuses one with nothing but context, which
/// must not start a run, and an image whose data is not the PNG, JPEG, GIF,
/// or WebP its `mime_type` declares, which a provider would refuse
/// mid-turn.
pub(crate) fn prompt(blocks: &[ContentBlock]) -> Result<Prompt, InvalidPrompt> {
    let mut person = Vec::with_capacity(blocks.len());
    let mut context = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        match block {
            ContentBlock::Context { text } => context.push(text.clone()),
            block => person.push(model_block(index, block)?),
        }
    }
    // A prompt with no message is the only one it refuses.
    let person = UserInput::content(person).map_err(|_| InvalidPrompt::Empty)?;
    Ok(Prompt { person, context })
}

/// Block `index` of the prompt as the model gets it.
fn model_block(index: usize, block: &ContentBlock) -> Result<model::ContentBlock, InvalidPrompt> {
    Ok(match block {
        ContentBlock::Text { text } | ContentBlock::Context { text } => {
            model::ContentBlock::Text(text.clone())
        }
        ContentBlock::Image { mime_type, data } => {
            model::ContentBlock::Image(image(index, mime_type, data)?)
        }
        ContentBlock::Resource { uri, text } => {
            model::ContentBlock::Text(fence_context("resource", uri, text))
        }
        ContentBlock::ResourceLink {
            uri,
            name,
            description,
        } => {
            let body = description.as_deref().unwrap_or(name);
            model::ContentBlock::Text(fence_context("resource", uri, body))
        }
        // `Unknown`, and the blocks of a newer protocol (`ContentBlock` is
        // non-exhaustive): the model still learns something was there.
        _ => model::ContentBlock::Text(fence_context("context", "unsupported", "")),
    })
}

/// Image block `index`, once its bytes are an image of the declared type
/// within the image limit.
fn image(index: usize, mime_type: &str, data: &str) -> Result<ImageContent, InvalidPrompt> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| InvalidPrompt::ImageNotBase64 { index })?;
    if bytes.len() as u64 > MAX_IMAGE_FILE_BYTES {
        return Err(InvalidPrompt::ImageTooLarge {
            index,
            bytes: bytes.len(),
        });
    }
    let actual = ImageContent::mime_type_from_bytes(&bytes)
        .ok_or(InvalidPrompt::ImageUnrecognized { index })?;
    if !mime_type.eq_ignore_ascii_case(actual) {
        return Err(InvalidPrompt::ImageMismatch {
            index,
            declared: mime_type.to_owned(),
            actual,
        });
    }
    Ok(ImageContent {
        data: data.to_owned(),
        mime_type: actual.to_owned(),
    })
}

/// `body` in a code fence whose info string is `kind header`. The fence
/// grows past the longest backtick run in either, so a body or URI that
/// holds its own fence cannot close it early.
pub(crate) fn fence_context(kind: &str, header: &str, body: &str) -> String {
    let fence = "`".repeat(
        longest_backtick_run(header)
            .max(longest_backtick_run(body))
            .max(2)
            + 1,
    );
    format!("{fence}{kind} {header}\n{body}\n{fence}")
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        run = if character == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

#[cfg(test)]
#[path = "prompt_content_tests.rs"]
mod tests;
