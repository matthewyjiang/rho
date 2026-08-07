//! Renders session history as readable text for the advisor model.
//!
//! The renderer is pure: it takes the executor system prompt and a message
//! slice and returns one string. Size control happens in two stages. Oversized
//! single items (system prompt, tool arguments, tool results) are clipped where
//! they are written, then the whole transcript is elided in the middle so the
//! advisor keeps both the opening of the session and the most recent work.

use rho_sdk::model::{ContentBlock, Message, ToolCall};

/// Size limits applied while rendering one transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptBudget {
    /// Rendered message body size before middle elision.
    pub(crate) body_bytes: usize,
    /// Executor system prompt size.
    pub(crate) system_prompt_bytes: usize,
    /// Arguments of one tool call.
    pub(crate) tool_call_bytes: usize,
    /// Content of one tool result.
    pub(crate) tool_result_bytes: usize,
}

pub(crate) const DEFAULT_TRANSCRIPT_BUDGET: TranscriptBudget = TranscriptBudget {
    body_bytes: 120_000,
    system_prompt_bytes: 20_000,
    tool_call_bytes: 2_000,
    tool_result_bytes: 4_000,
};

/// Bytes held back from [`TranscriptBudget::body_bytes`] for the elision mark,
/// so an elided body still fits the requested budget.
const ELISION_MARK_RESERVE: usize = 96;

pub(crate) fn render_transcript(
    system_prompt: Option<&str>,
    messages: &[Message],
    budget: TranscriptBudget,
) -> String {
    let mut rendered = String::new();
    if let Some(prompt) = system_prompt {
        rendered.push_str("# Executor system prompt\n\n");
        push_clipped(&mut rendered, prompt, budget.system_prompt_bytes);
        rendered.push_str("\n\n");
    }
    rendered.push_str("# Session transcript\n");
    if messages.is_empty() {
        rendered.push_str("\nThe session has no messages yet.\n");
        return rendered;
    }
    let mut body = String::new();
    for message in messages {
        push_message(&mut body, message, budget);
    }
    rendered.push_str(&elide_middle(body, budget.body_bytes));
    rendered
}

fn push_message(out: &mut String, message: &Message, budget: TranscriptBudget) {
    match message {
        Message::System(text) => {
            out.push_str("\n## system\n\n");
            push_clipped(out, text, budget.system_prompt_bytes);
            out.push('\n');
        }
        Message::User(blocks) => {
            out.push_str("\n## user\n\n");
            push_blocks(out, blocks, budget);
        }
        Message::Assistant(blocks) => {
            out.push_str("\n## assistant\n\n");
            push_blocks(out, blocks, budget);
        }
        Message::EnrichedAssistant(assistant) => {
            out.push_str("\n## assistant\n\n");
            push_blocks(out, &assistant.content, budget);
        }
        Message::AbortedAssistant(aborted) => {
            out.push_str("\n## assistant (interrupted)\n\n");
            push_blocks(out, &aborted.content, budget);
            for call in &aborted.tool_calls {
                let name = call.name.as_deref().unwrap_or("unknown");
                out.push_str("tool call (incomplete): ");
                out.push_str(name);
                out.push_str("\narguments: ");
                push_clipped(out, &call.arguments, budget.tool_call_bytes);
                out.push('\n');
            }
        }
        Message::ToolResult(result) => {
            let status = if result.ok { "ok" } else { "error" };
            out.push_str("\n## tool result ");
            out.push_str(&result.id);
            out.push_str(" (");
            out.push_str(status);
            out.push_str(")\n\n");
            push_clipped(out, &result.content, budget.tool_result_bytes);
            out.push('\n');
        }
    }
}

fn push_blocks(out: &mut String, blocks: &[ContentBlock], budget: TranscriptBudget) {
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                out.push_str(text.trim_end());
                out.push('\n');
            }
            ContentBlock::Image(image) => {
                out.push_str("[image: ");
                out.push_str(&image.mime_type);
                out.push_str("]\n");
            }
            ContentBlock::ToolCall(call) => push_tool_call(out, call, budget.tool_call_bytes),
        }
    }
}

fn push_tool_call(out: &mut String, call: &ToolCall, max_bytes: usize) {
    out.push_str("tool call: ");
    out.push_str(&call.name);
    out.push_str(" (id ");
    out.push_str(&call.id);
    out.push_str(")\narguments: ");
    push_clipped(out, &call.arguments.to_string(), max_bytes);
    out.push('\n');
}

fn push_clipped(out: &mut String, text: &str, max_bytes: usize) {
    if text.len() <= max_bytes {
        out.push_str(text);
        return;
    }
    let end = rho_sdk::floor_char_boundary(text, max_bytes);
    out.push_str(&text[..end]);
    out.push_str(&format!("\n[... {} bytes clipped ...]", text.len() - end));
}

/// Keeps the opening and the most recent portion of an oversized body.
///
/// The most recent work matters most for advice, so the tail keeps the larger
/// share; the head keeps enough of the opening to carry the original request.
fn elide_middle(body: String, max_bytes: usize) -> String {
    if body.len() <= max_bytes {
        return body;
    }
    let available = max_bytes.saturating_sub(ELISION_MARK_RESERVE);
    let head_bytes = available * 2 / 5;
    let head_end = line_floor(&body, head_bytes);
    let tail_start = line_ceil(&body, body.len() - (available - head_bytes));
    if tail_start <= head_end {
        return body;
    }
    let mut elided = String::with_capacity(max_bytes);
    elided.push_str(&body[..head_end]);
    elided.push_str(&format!(
        "\n\n[... {} bytes of the middle of the session elided ...]\n\n",
        tail_start - head_end
    ));
    elided.push_str(&body[tail_start..]);
    elided
}

/// Last line start at or before `index`, so a head cut lands between lines.
fn line_floor(text: &str, index: usize) -> usize {
    let index = rho_sdk::floor_char_boundary(text, index);
    text[..index]
        .rfind('\n')
        .map(|position| position + 1)
        .unwrap_or(index)
}

/// First line start at or after `index`, so a tail cut lands between lines.
fn line_ceil(text: &str, index: usize) -> usize {
    let index = rho_sdk::ceil_char_boundary(text, index);
    text[index..]
        .find('\n')
        .map(|position| index + position + 1)
        .unwrap_or(index)
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
