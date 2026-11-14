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
//! model gets ahead of the person's message but the session does not save
//! as part of it. Only
//! whole, known tags at the start of the first text block or the end of the
//! last one count: a tag mid-text, or one this module does not know, stays
//! the person's.

use rho_protocol::ContentBlock;

/// T3's instructions, which lead a prompt that states them.
const INSTRUCTIONS: &str = "t3_code_instructions";
/// What wraps the person's text after T3's instructions.
const REQUEST: &str = "user_request";
/// The blocks T3 ends every prompt with, in order.
const TRAILING: [&str; 2] = ["runtime_info", "pull_request_linking"];

/// `prompt` with T3's wrapping as context blocks, ahead of what the person
/// sent, as the model gets context: its instructions, then its trailing
/// blocks. A text block that held nothing else is dropped.
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
    let trailing = text_mut(&mut prompt[last])
        .map(take_trailing)
        .unwrap_or_default();
    let leading = text_mut(&mut prompt[first]).and_then(take_instructions);
    if leading.is_none() && trailing.is_empty() {
        return prompt;
    }
    let emptied = |index: usize, block: &ContentBlock| {
        (index == first || index == last)
            && matches!(block, ContentBlock::Text { text } if text.is_empty())
    };
    let person = prompt
        .into_iter()
        .enumerate()
        .filter(|(index, block)| !emptied(*index, block))
        .map(|(_, block)| block);
    leading
        .into_iter()
        .chain(trailing)
        .map(|text| ContentBlock::Context { text })
        .chain(person)
        .collect()
}

fn text_mut(block: &mut ContentBlock) -> Option<&mut String> {
    match block {
        ContentBlock::Text { text } => Some(text),
        _ => None,
    }
}

/// Removes the trailing blocks T3 ends `text` with, at most one of each
/// in their order, and returns them in order. A block that repeats one of
/// them, or comes out of order, stays the person's.
fn take_trailing(text: &mut String) -> Vec<String> {
    let mut taken = Vec::new();
    for tag in TRAILING.iter().rev() {
        let end = text.trim_end().len();
        if let Some(start) = trailing_block(&text[..end], tag) {
            taken.push(text[start..end].to_owned());
            text.truncate(start);
        }
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
    let instructions = text[..end].to_owned();
    let rest = text[end..].trim_start();
    *text = unwrap_request(rest).unwrap_or(rest).to_owned();
    Some(instructions)
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
