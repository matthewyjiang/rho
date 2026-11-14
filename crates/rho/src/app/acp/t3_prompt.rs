//! T3 Code's wrapping of the prompts it sends `rho acp --t3code`.
//!
//! T3 puts its own tagged blocks around what the person typed, with any
//! images between:
//!
//! - A prompt that states T3's instructions, as a thread's first does,
//!   starts its first text block with
//!   `<t3_code_instructions>…</t3_code_instructions>` and wraps the
//!   person's text in `<user_request>…</user_request>`.
//! - Every prompt ends with a text block of `<runtime_info>…</runtime_info>`
//!   and `<pull_request_linking>…</pull_request_linking>`.
//!
//! [`split_context`] turns that wrapping into context blocks, which the
//! model gets but the session does not save as the person's message. Only
//! whole, known tags at the start of the first text block or the end of the
//! last one count: a tag mid-text, or one this module does not know, stays
//! the person's.

use rho_protocol::ContentBlock;

/// T3's instructions, which lead a prompt that states them.
const INSTRUCTIONS: &str = "t3_code_instructions";
/// What wraps the person's text after T3's instructions.
const REQUEST: &str = "user_request";
/// The blocks T3 ends every prompt with.
const TRAILING: [&str; 2] = ["runtime_info", "pull_request_linking"];

/// `prompt` with T3's wrapping as context blocks: its instructions first,
/// then what the person sent, then the trailing blocks. A text block left
/// empty is dropped.
pub(super) fn split_context(mut prompt: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let is_text = |block: &ContentBlock| matches!(block, ContentBlock::Text { .. });
    let (Some(first), Some(last)) = (
        prompt.iter().position(is_text),
        prompt.iter().rposition(is_text),
    ) else {
        return prompt;
    };
    // The end first: when one block holds everything, the person's text
    // ends with `</user_request>` only once the trailing blocks are gone.
    let trailing = match &mut prompt[last] {
        ContentBlock::Text { text } => take_trailing(text),
        _ => Vec::new(),
    };
    let leading = match &mut prompt[first] {
        ContentBlock::Text { text } => take_instructions(text),
        _ => None,
    };
    if leading.is_none() && trailing.is_empty() {
        return prompt;
    }
    let context = |text| ContentBlock::Context { text };
    leading
        .map(context)
        .into_iter()
        .chain(prompt.into_iter().filter(
            |block| !matches!(block, ContentBlock::Text { text } if text.trim().is_empty()),
        ))
        .chain(trailing.into_iter().map(context))
        .collect()
}

/// Removes the known blocks `text` ends with and returns them in order.
fn take_trailing(text: &mut String) -> Vec<String> {
    let mut taken = Vec::new();
    loop {
        let end = text.trim_end().len();
        let Some(start) = TRAILING
            .iter()
            .find_map(|tag| trailing_block(&text[..end], tag))
        else {
            break;
        };
        taken.push(text[start..end].to_owned());
        text.truncate(start);
    }
    if !taken.is_empty() {
        // The break that set them apart is T3's, not the person's.
        text.truncate(text.trim_end().len());
    }
    taken.reverse();
    taken
}

/// Where the `tag` block `text` ends with starts: its opening tag begins
/// the text or a line.
fn trailing_block(text: &str, tag: &str) -> Option<usize> {
    if !text.ends_with(&format!("</{tag}>")) {
        return None;
    }
    let start = text.rfind(&format!("<{tag}>"))?;
    (start == 0 || text[..start].ends_with('\n')).then_some(start)
}

/// Removes the instructions block `text` starts with, and the
/// `<user_request>` wrapping of what follows, and returns the block.
fn take_instructions(text: &mut String) -> Option<String> {
    let open = format!("<{INSTRUCTIONS}>");
    let close = format!("</{INSTRUCTIONS}>");
    let end = open.len() + text.strip_prefix(&open)?.find(&close)? + close.len();
    let rest = text[end..].trim_start();
    let person = unwrap_request(rest).unwrap_or(rest).to_owned();
    let instructions = std::mem::replace(text, person);
    Some(instructions[..end].to_owned())
}

/// The text `<user_request>` wraps when it wraps all of `text`, without the
/// line break on either side of it.
fn unwrap_request(text: &str) -> Option<&str> {
    let inner = text
        .trim_end()
        .strip_prefix(&format!("<{REQUEST}>"))?
        .strip_suffix(&format!("</{REQUEST}>"))?;
    let inner = inner.strip_prefix('\n').unwrap_or(inner);
    Some(inner.strip_suffix('\n').unwrap_or(inner))
}

#[cfg(test)]
#[path = "t3_prompt_tests.rs"]
mod tests;
