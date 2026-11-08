//! A prompt's content blocks as the user input its turn runs: the one
//! conversion behind the daemon's `rho/session/prompt` and ACP's
//! `session/prompt` (#1670).
//!
//! Text and images stay typed. Resources, resource links, and blocks rho
//! cannot read become fenced text, labelled with their kind and URI, so the
//! model still sees them.

use rho_protocol::ContentBlock;
use rho_sdk::{
    model::{self, ImageContent},
    UserInput,
};

/// The user input `blocks` make; an error when there are none, since an
/// empty prompt must not start a run.
pub(crate) fn user_input(blocks: &[ContentBlock]) -> anyhow::Result<UserInput> {
    Ok(UserInput::content(
        blocks.iter().map(model_block).collect(),
    )?)
}

fn model_block(block: &ContentBlock) -> model::ContentBlock {
    match block {
        ContentBlock::Text { text } => model::ContentBlock::Text(text.clone()),
        ContentBlock::Image { mime_type, data } => model::ContentBlock::Image(ImageContent {
            data: data.clone(),
            mime_type: mime_type.clone(),
        }),
        ContentBlock::Resource { uri, text, .. } => {
            model::ContentBlock::Text(fence_context("resource", uri, text))
        }
        ContentBlock::ResourceLink {
            uri,
            name,
            description,
            ..
        } => {
            let body = description.as_deref().unwrap_or(name);
            model::ContentBlock::Text(fence_context("resource", uri, body))
        }
        // `Unknown`, and the blocks of a newer protocol (`ContentBlock` is
        // non-exhaustive): the model still learns something was there.
        _ => model::ContentBlock::Text(fence_context("context", "unsupported", "")),
    }
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
