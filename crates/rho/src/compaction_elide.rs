//! First compaction tier: replace old tool-result bodies with recallable stubs.
//!
//! Elision keeps every `ToolResult` message and its `tool_call_id`, so call and
//! result pairing stays valid for every provider. Only the content changes. The
//! original content stays in the session transcript and is addressed by a
//! [`recall_id`] derived from it, which is stable across resume and branches.

use std::{collections::BTreeMap, ops::Range};

use rho_providers::model::{
    context::{estimate_context_tokens, estimate_message_tokens},
    Message,
};
use rho_sdk::model::ContentBlock;
use rho_tools::tool::{ToolResult, ToolSpec};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::partition_messages_for_compaction;

/// Results below this size stay verbatim. Measured over 50,162 tool results in
/// 600 local sessions: results of at least 1 KiB are 62% of results but 95% of
/// result bytes, so eliding smaller ones saves little and costs recall trips.
const MIN_ELIDED_BYTES: usize = 1024;
/// Argument summaries identify the call (path, command, pattern), not replay it.
const ARGUMENT_SUMMARY_CHARS: usize = 80;

/// History after elision and the originals it replaced, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Elision {
    pub messages: Vec<Message>,
    pub originals: Vec<ToolResult>,
}

/// Stable handle for one persisted tool result. Hashing the call id with the
/// content keeps ids unique even when providers reuse call ids like `call_0`.
/// Images are hashed only when present, so text-only ids match the ones saved
/// before results carried images.
pub(crate) fn recall_id(result: &ToolResult) -> String {
    let mut digest = Sha256::new();
    digest.update(result.id.as_bytes());
    digest.update([0]);
    digest.update(result.content.as_bytes());
    for image in &result.images {
        digest.update([0]);
        digest.update(image.mime_type.as_bytes());
        digest.update([0]);
        digest.update(image.data.as_bytes());
    }
    let digest = hex::encode(digest.finalize());
    format!("r{}", &digest[..16])
}

/// Elides the oldest eligible tool results outside the recent tail until the
/// local estimate reaches `target_tokens`, or until none are left. Returns
/// `None` when nothing was elided. The tail uses the same token budget as
/// text-summary compaction and is never modified.
pub(crate) fn elide_tool_results(
    messages: &[Message],
    tools: &[ToolSpec],
    target_tokens: u64,
) -> Option<Elision> {
    let partition = partition_messages_for_compaction(messages, tools, target_tokens)?;
    let Range { start, end } = partition.compacted_range();
    let owners = Owners::new(&messages[start..end], start);

    let mut stubs = BTreeMap::new();
    let mut tokens = estimate_context_tokens(messages, tools);
    for (index, message) in messages.iter().enumerate().take(end).skip(start) {
        if tokens <= target_tokens {
            break;
        }
        let Message::ToolResult(result) = message else {
            continue;
        };
        if result.content.len() < MIN_ELIDED_BYTES && result.images.is_empty() {
            continue;
        }
        // The stub drops the images; the original keeps them for recall.
        let stub = Message::ToolResult(ToolResult::new(
            result.id.clone(),
            result.status,
            stub_text(result, owners.call(index)),
        ));
        tokens = tokens
            .saturating_sub(estimate_message_tokens(message))
            .saturating_add(estimate_message_tokens(&stub));
        stubs.insert(index, stub);
    }
    if stubs.is_empty() {
        return None;
    }
    let mut originals = Vec::with_capacity(stubs.len());
    let messages = messages
        .iter()
        .enumerate()
        .map(|(index, message)| match stubs.remove(&index) {
            Some(stub) => {
                if let Message::ToolResult(result) = message {
                    originals.push(result.clone());
                }
                stub
            }
            None => message.clone(),
        })
        .collect();
    Some(Elision {
        messages,
        originals,
    })
}

struct CallSummary<'a> {
    name: &'a str,
    arguments: &'a Value,
}

/// Positional ownership inside the compacted range. Providers may reuse call
/// ids across turns (`call_0`), so ids alone are ambiguous: a result belongs to
/// the nearest preceding call with its id.
struct Owners<'a> {
    calls: BTreeMap<usize, CallSummary<'a>>,
}

impl<'a> Owners<'a> {
    fn new(range: &'a [Message], offset: usize) -> Self {
        let mut latest_call = BTreeMap::<&str, CallSummary<'a>>::new();
        let mut owners = Self {
            calls: BTreeMap::new(),
        };
        for (position, message) in range.iter().enumerate() {
            let index = offset + position;
            if let Message::ToolResult(result) = message {
                if let Some(call) = latest_call.get(result.id.as_str()) {
                    owners.calls.insert(
                        index,
                        CallSummary {
                            name: call.name,
                            arguments: call.arguments,
                        },
                    );
                }
            }
            for block in message.completed_assistant_content().into_iter().flatten() {
                if let ContentBlock::ToolCall(call) = block {
                    latest_call.insert(
                        &call.id,
                        CallSummary {
                            name: &call.name,
                            arguments: &call.arguments,
                        },
                    );
                }
            }
        }
        owners
    }

    fn call(&self, result_index: usize) -> Option<&CallSummary<'a>> {
        self.calls.get(&result_index)
    }
}

fn stub_text(result: &ToolResult, call: Option<&CallSummary<'_>>) -> String {
    let call = match call {
        Some(call) => {
            let arguments = argument_summary(call.arguments);
            if arguments.is_empty() {
                call.name.to_owned()
            } else {
                format!("{} {arguments}", call.name)
            }
        }
        None => "unknown".to_owned(),
    };
    let status = if result.is_failure() { "error" } else { "ok" };
    let images = match result.images.len() {
        0 => String::new(),
        1 => " + 1 image".to_owned(),
        count => format!(" + {count} images"),
    };
    format!(
        "[elided tool result: {call} · {status} · {} bytes{images} · recall_id={}; fetch the text with the sessions tool, action=recall]",
        result.content.len(),
        recall_id(result),
    )
}

fn argument_summary(arguments: &Value) -> String {
    let summary = match arguments {
        Value::Object(fields) => fields
            .iter()
            .filter_map(|(key, value)| match value {
                Value::String(text) => Some(format!("{key}={text}")),
                Value::Number(number) => Some(format!("{key}={number}")),
                Value::Bool(flag) => Some(format!("{key}={flag}")),
                Value::Null | Value::Array(_) | Value::Object(_) => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() <= ARGUMENT_SUMMARY_CHARS {
        return summary;
    }
    let mut truncated: String = summary.chars().take(ARGUMENT_SUMMARY_CHARS - 1).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "compaction_elide_tests.rs"]
mod tests;
