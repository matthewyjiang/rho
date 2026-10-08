//! Itemized local context estimate, so hosts can attribute the window by source.

use std::collections::HashMap;

use crate::{
    model::{
        context::{estimate_message_tokens, tool_spec_tokens, REQUEST_OVERHEAD_TOKENS},
        ContentBlock, Message, SemanticMessage, ToolSpec,
    },
    ContextEstimate,
};

/// What one [`ContextPart`] measures.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContextItem {
    /// Fixed per-request framing outside every message and schema.
    RequestOverhead,
    /// One advertised tool schema.
    ToolSchema { name: String },
    /// The leading system message, which holds the session system prompt.
    SystemPrompt,
    /// A later system message in history.
    System,
    /// A user-role message that is neither a compaction summary nor a tool
    /// image supplement. This includes context the host appended as a user message.
    User,
    /// A compaction summary that replaced earlier history.
    CompactionSummary,
    /// Assistant output, including reasoning replay and tool call arguments.
    Assistant,
    /// A tool result or its image supplement. `tool` is `None` when no earlier
    /// assistant tool call carries the result's id.
    ToolResult { tool: Option<String> },
    /// One request-only message from the host's [`crate::RequestContext`],
    /// which is sent with each request but never stored in history.
    RequestContext,
}

/// Local token estimate for one item of the next request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextPart {
    item: ContextItem,
    tokens: u64,
}

impl ContextPart {
    pub fn item(&self) -> &ContextItem {
        &self.item
    }

    /// Provider-neutral estimate, on the same scale as
    /// [`ContextEstimate::estimated_tokens`].
    pub const fn tokens(&self) -> u64 {
        self.tokens
    }
}

/// The next request split into items, with the session's context estimate.
///
/// Every part uses the provider-neutral heuristic behind
/// [`ContextEstimate::estimated_tokens`], so the parts sum to exactly that value.
/// Provider calibration applies only to the total ([`ContextEstimate::tokens`]);
/// hosts that show calibrated per-source numbers must scale the parts themselves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextBreakdown {
    estimate: ContextEstimate,
    parts: Vec<ContextPart>,
}

impl ContextBreakdown {
    /// Parts are ordered: request overhead, tool schemas, history, request context.
    pub(crate) fn new(
        estimate: ContextEstimate,
        history: &[Message],
        request_context: &[Message],
        tools: &[ToolSpec],
    ) -> Self {
        let mut parts = Vec::with_capacity(1 + tools.len() + history.len() + request_context.len());
        parts.push(ContextPart {
            item: ContextItem::RequestOverhead,
            tokens: REQUEST_OVERHEAD_TOKENS,
        });
        parts.extend(tools.iter().map(|spec| ContextPart {
            item: ContextItem::ToolSchema {
                name: spec.name.clone(),
            },
            tokens: tool_spec_tokens(spec),
        }));
        let mut call_names = HashMap::new();
        for (index, message) in history.iter().enumerate() {
            let item = match message.semantic() {
                SemanticMessage::System(_) if index == 0 => ContextItem::SystemPrompt,
                SemanticMessage::System(_) => ContextItem::System,
                SemanticMessage::User(_) if message.as_compaction_summary().is_some() => {
                    ContextItem::CompactionSummary
                }
                SemanticMessage::User(_) => ContextItem::User,
                SemanticMessage::Assistant(content) => {
                    record_calls(&mut call_names, content);
                    ContextItem::Assistant
                }
                SemanticMessage::EnrichedAssistant(message) => {
                    record_calls(&mut call_names, &message.content);
                    ContextItem::Assistant
                }
                SemanticMessage::AbortedAssistant(message) => {
                    record_calls(&mut call_names, &message.content);
                    ContextItem::Assistant
                }
                SemanticMessage::ToolResult(result) => ContextItem::ToolResult {
                    tool: call_names.get(result.id.as_str()).cloned(),
                },
                SemanticMessage::ToolImageSupplement(supplement) => ContextItem::ToolResult {
                    tool: Some(supplement.tool_name().to_owned()),
                },
            };
            parts.push(ContextPart {
                item,
                tokens: estimate_message_tokens(message),
            });
        }
        parts.extend(request_context.iter().map(|message| ContextPart {
            item: ContextItem::RequestContext,
            tokens: estimate_message_tokens(message),
        }));
        Self { estimate, parts }
    }

    /// The session estimate for the same request, calibrated when applicable.
    pub const fn estimate(&self) -> ContextEstimate {
        self.estimate
    }

    pub fn parts(&self) -> &[ContextPart] {
        &self.parts
    }
}

fn record_calls<'a>(names: &mut HashMap<&'a str, String>, content: &'a [ContentBlock]) {
    for block in content {
        if let ContentBlock::ToolCall(call) = block {
            names.insert(call.id.as_str(), call.name.clone());
        }
    }
}

#[cfg(test)]
#[path = "context_breakdown_tests.rs"]
mod tests;
