//! Pair late tool results for providers that require adjacency.
//!
//! OpenAI Responses accepts a `function_call_output` later in the conversation.
//! Anthropic, Gemini, and chat-completions require each tool result to follow
//! its assistant call immediately. When a result is delayed, insert a
//! placeholder beside the call and rewrite the real result as user text.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use crate::model::{ContentBlock, Message, ToolResult};

const LATE_PLACEHOLDER: &str = "result delivered later in this conversation";

/// Returns history where every completed assistant tool call is followed by an
/// adjacent `ToolResult`, rewriting delayed results into user text.
pub(crate) fn normalize_late_tool_results(messages: &[Message]) -> Cow<'_, [Message]> {
    if !needs_normalize(messages) {
        return Cow::Borrowed(messages);
    }

    let mut out = Vec::with_capacity(messages.len());
    let mut late_names = BTreeMap::<String, String>::new();
    let mut index = 0;
    while index < messages.len() {
        if let Message::ToolResult(result) = &messages[index] {
            if let Some(name) = late_names.remove(&result.id) {
                out.push(late_result_user_message(&result.id, &name, &result.content));
                index += 1;
                continue;
            }
        }

        let calls = assistant_tool_calls(&messages[index]);
        out.push(messages[index].clone());
        index += 1;
        if calls.is_empty() {
            continue;
        }

        let mut adjacent_ids = BTreeSet::new();
        while index < messages.len() {
            let Message::ToolResult(result) = &messages[index] else {
                break;
            };
            if late_names.contains_key(&result.id) {
                break;
            }
            adjacent_ids.insert(result.id.as_str());
            out.push(messages[index].clone());
            index += 1;
        }

        for (id, name) in calls {
            if adjacent_ids.contains(id) {
                continue;
            }
            out.push(Message::ToolResult(ToolResult {
                id: id.to_owned(),
                ok: true,
                content: LATE_PLACEHOLDER.into(),
            }));
            late_names.insert(id.to_owned(), name.to_owned());
        }
    }

    Cow::Owned(out)
}

fn needs_normalize(messages: &[Message]) -> bool {
    let mut index = 0;
    while index < messages.len() {
        let calls = assistant_tool_calls(&messages[index]);
        index += 1;
        if calls.is_empty() {
            continue;
        }
        let mut adjacent_ids = BTreeSet::new();
        while index < messages.len() {
            let Message::ToolResult(result) = &messages[index] else {
                break;
            };
            adjacent_ids.insert(result.id.as_str());
            index += 1;
        }
        if calls.iter().any(|(id, _)| !adjacent_ids.contains(id)) {
            return true;
        }
    }
    false
}

fn assistant_tool_calls(message: &Message) -> Vec<(&str, &str)> {
    let blocks = match message {
        Message::Assistant(blocks) => blocks.as_slice(),
        Message::EnrichedAssistant(message) => message.content.as_slice(),
        Message::System(_)
        | Message::User(_)
        | Message::AbortedAssistant(_)
        | Message::ToolResult(_) => return Vec::new(),
    };
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some((call.id.as_str(), call.name.as_str())),
            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect()
}

fn late_result_user_message(id: &str, name: &str, content: &str) -> Message {
    Message::user_text(format!("Result for tool call {id} ({name}): {content}"))
}

#[cfg(test)]
#[path = "late_tool_results_tests.rs"]
mod tests;
