//! First compaction tier: replace old tool-result bodies with recallable stubs.
//!
//! Elision keeps every `ToolResult` message and its `tool_call_id`, so call and
//! result pairing stays valid for every provider. Only the content changes. The
//! original content stays in the session transcript and is addressed by a
//! [`recall_id`] derived from it, which is stable across resume and branches.

use std::collections::BTreeMap;

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
pub(crate) fn recall_id(result: &ToolResult) -> String {
    let mut digest = Sha256::new();
    digest.update(result.id.as_bytes());
    digest.update([0]);
    digest.update(result.content.as_bytes());
    let digest = format!("{:x}", digest.finalize());
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
    // Anchors and an earlier summary may precede the compacted span, so locate
    // it from the end.
    let end = messages.len() - partition.recent_messages.len();
    let start = end - partition.compacted_messages.len();
    let owners = Owners::new(&messages[start..end], start);

    let mut output = messages.to_vec();
    let mut dropped = vec![false; messages.len()];
    let mut tokens = estimate_context_tokens(messages, tools);
    let mut originals = Vec::new();
    for index in start..end {
        if tokens <= target_tokens {
            break;
        }
        let Message::ToolResult(result) = &messages[index] else {
            continue;
        };
        let images = owners.supplements(index);
        if result.content.len() < MIN_ELIDED_BYTES && images.is_empty() {
            continue;
        }
        let image_count = images
            .iter()
            .filter_map(|&index| messages[index].as_tool_image_supplement())
            .map(|supplement| supplement.images().len())
            .sum();
        let stub = Message::ToolResult(ToolResult {
            id: result.id.clone(),
            ok: result.ok,
            content: stub_text(result, owners.call(index), image_count),
        });
        tokens = tokens
            .saturating_sub(estimate_message_tokens(&messages[index]))
            .saturating_add(estimate_message_tokens(&stub));
        for &image_index in images {
            tokens = tokens.saturating_sub(estimate_message_tokens(&messages[image_index]));
            dropped[image_index] = true;
        }
        output[index] = stub;
        originals.push(result.clone());
    }
    (!originals.is_empty()).then(|| Elision {
        messages: output
            .into_iter()
            .zip(dropped)
            .filter_map(|(message, dropped)| (!dropped).then_some(message))
            .collect(),
        originals,
    })
}

struct CallSummary<'a> {
    name: &'a str,
    arguments: &'a Value,
}

/// Positional ownership inside the compacted range. Providers may reuse call
/// ids across turns (`call_0`), so ids alone are ambiguous: a result belongs to
/// the nearest preceding call with its id, and an image supplement to the
/// nearest preceding result for its call id.
struct Owners<'a> {
    calls: BTreeMap<usize, CallSummary<'a>>,
    supplements: BTreeMap<usize, Vec<usize>>,
}

impl<'a> Owners<'a> {
    fn new(range: &'a [Message], offset: usize) -> Self {
        let mut latest_call = BTreeMap::<&str, CallSummary<'a>>::new();
        let mut latest_result = BTreeMap::<&str, usize>::new();
        let mut owners = Self {
            calls: BTreeMap::new(),
            supplements: BTreeMap::new(),
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
                latest_result.insert(&result.id, index);
            } else if let Some(images) = message.as_tool_image_supplement() {
                if let Some(&owner) = latest_result.get(images.tool_call_id()) {
                    owners.supplements.entry(owner).or_default().push(index);
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

    fn supplements(&self, result_index: usize) -> &[usize] {
        self.supplements
            .get(&result_index)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

fn stub_text(result: &ToolResult, call: Option<&CallSummary<'_>>, image_count: usize) -> String {
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
    let status = if result.ok { "ok" } else { "error" };
    let images = match image_count {
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
