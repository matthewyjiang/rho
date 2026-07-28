use std::collections::BTreeMap;

use crate::{
    model::{
        AbortedAssistant, ContentBlock, ModelEvent, ModelUsage, PartialToolCall,
        ProviderContextBlock, ToolCall,
    },
    ProviderErrorKind, RunEvent,
};

#[derive(Default)]
pub(super) struct StreamCapture {
    content: Vec<ContentBlock>,
    merge_output_text: bool,
    /// When set, the next text block is sealed and must not absorb later deltas.
    /// Used after provider-context boundaries such as Gemini thought signatures.
    seal_next_text_part: bool,
    /// Maps provider tool-call stream indexes onto `content` positions.
    tool_call_content_index: BTreeMap<usize, usize>,
    reasoning: String,
    reasoning_summary: String,
    provider_context: Vec<ProviderContextBlock>,
    partial_tool_calls: BTreeMap<usize, CapturedToolCall>,
    usage: ModelUsage,
    failed_attempts: Vec<(ProviderErrorKind, ModelUsage)>,
}

impl StreamCapture {
    pub(super) fn usage(&self) -> &ModelUsage {
        &self.usage
    }

    pub(super) fn take_failed_attempts(&mut self) -> Vec<(ProviderErrorKind, ModelUsage)> {
        std::mem::take(&mut self.failed_attempts)
    }

    pub(super) fn record_request_attempt_failure(
        &mut self,
        kind: ProviderErrorKind,
        usage: ModelUsage,
    ) {
        let attempt_usage = self.usage.saturating_add(&usage);
        self.usage = ModelUsage::default();
        self.failed_attempts.push((kind, attempt_usage));
    }

    pub(super) fn take_assistant_context(&mut self) -> (Option<String>, Vec<ProviderContextBlock>) {
        let summary = (!self.reasoning_summary.is_empty())
            .then(|| std::mem::take(&mut self.reasoning_summary));
        let provider_context = std::mem::take(&mut self.provider_context);
        (summary, provider_context)
    }

    pub(super) fn into_aborted_assistant(self) -> Option<AbortedAssistant> {
        if self.content.is_empty()
            && self.reasoning_summary.is_empty()
            && self.provider_context.is_empty()
            && self.partial_tool_calls.is_empty()
            && self.usage == ModelUsage::default()
        {
            return None;
        }
        Some(AbortedAssistant {
            content: self.content,
            reasoning: String::new(),
            provenance: None,
            reasoning_summary: (!self.reasoning_summary.is_empty())
                .then_some(self.reasoning_summary),
            provider_context: self.provider_context,
            // Keep fragments for provider fallbacks even when complete calls were also
            // placed into `content` to preserve stream positions.
            tool_calls: self
                .partial_tool_calls
                .into_values()
                .map(|call| call.partial)
                .collect(),
            usage: self.usage,
        })
    }
}

struct CapturedToolCall {
    partial: PartialToolCall,
    arguments: JsonObjectCompletion,
    parsed_arguments: Option<serde_json::Value>,
}

impl Default for CapturedToolCall {
    fn default() -> Self {
        Self {
            partial: PartialToolCall {
                id: None,
                name: None,
                arguments: String::new(),
            },
            arguments: JsonObjectCompletion::default(),
            parsed_arguments: None,
        }
    }
}

/// Tracks whether an append-only byte stream has closed one top-level JSON object.
///
/// This is only a completion detector. `serde_json` remains the source of truth
/// for syntax and object validation when the outer object first closes.
#[derive(Default)]
struct JsonObjectCompletion {
    depth: usize,
    started: bool,
    in_string: bool,
    escaped: bool,
    complete: bool,
}

impl JsonObjectCompletion {
    fn push(&mut self, fragment: &str) -> bool {
        if self.complete {
            return false;
        }
        for byte in fragment.bytes() {
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                }
                continue;
            }

            match byte {
                b'{' if !self.started => {
                    self.started = true;
                    self.depth = 1;
                }
                b'"' if self.started => self.in_string = true,
                b'{' | b'[' if self.started => self.depth += 1,
                b'}' | b']' if self.started && self.depth > 0 => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        self.complete = true;
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
}

fn upsert_captured_tool_call(capture: &mut StreamCapture, index: usize) {
    let content_index = capture.tool_call_content_index.get(&index).copied();
    let Some(captured) = capture.partial_tool_calls.get_mut(&index) else {
        return;
    };
    let partial = &captured.partial;
    let Some(id) = partial.id.as_ref().filter(|id| !id.is_empty()).cloned() else {
        return;
    };
    let Some(name) = partial
        .name
        .as_ref()
        .filter(|name| !name.is_empty())
        .cloned()
    else {
        return;
    };
    if let Some(content_index) = content_index {
        if let Some(ContentBlock::ToolCall(call)) = capture.content.get_mut(content_index) {
            call.id = id;
            call.name = name;
        }
        return;
    }
    let Some(arguments) = captured.parsed_arguments.take() else {
        return;
    };
    let call = ContentBlock::ToolCall(ToolCall {
        id,
        name,
        arguments,
    });
    let content_index = capture.content.len();
    capture.tool_call_content_index.insert(index, content_index);
    capture.content.push(call);
}

pub(super) fn capture_provider_event(
    event: ModelEvent,
    identity: &crate::model::ModelIdentity,
    accumulated_usage: &ModelUsage,
    capture: &mut StreamCapture,
) -> RunEvent {
    match event {
        ModelEvent::OutputDelta(text) => {
            if capture.merge_output_text {
                let Some(ContentBlock::Text(existing)) = capture.content.last_mut() else {
                    capture.content.push(ContentBlock::Text(text.clone()));
                    capture.merge_output_text = !capture.seal_next_text_part;
                    capture.seal_next_text_part = false;
                    return RunEvent::AssistantTextDelta { text };
                };
                existing.push_str(&text);
            } else {
                capture.content.push(ContentBlock::Text(text.clone()));
                // Text that starts after provider-context (for example a Gemini
                // thought signature) must remain a standalone part.
                capture.merge_output_text = !capture.seal_next_text_part;
                capture.seal_next_text_part = false;
            }
            RunEvent::AssistantTextDelta { text }
        }
        ModelEvent::ReasoningDelta(text) => {
            capture.merge_output_text = false;
            capture.seal_next_text_part = false;
            capture.reasoning.push_str(&text);
            RunEvent::ReasoningDelta { text }
        }
        ModelEvent::ReasoningSummaryDelta(text) => {
            capture.merge_output_text = false;
            capture.seal_next_text_part = false;
            capture.reasoning_summary.push_str(&text);
            RunEvent::ReasoningSummaryDelta { text }
        }
        ModelEvent::WebSearch(detail) => RunEvent::WebSearch { detail },
        ModelEvent::ToolCallDelta {
            index,
            id,
            name,
            arguments,
        } => {
            capture.merge_output_text = false;
            capture.seal_next_text_part = false;
            let partial = capture.partial_tool_calls.entry(index).or_default();
            if id.is_some() {
                partial.partial.id.clone_from(&id);
            }
            if name.is_some() {
                partial.partial.name.clone_from(&name);
            }
            partial.partial.arguments.push_str(&arguments);
            if partial.arguments.push(&arguments) {
                partial.parsed_arguments =
                    serde_json::from_str::<serde_json::Value>(&partial.partial.arguments)
                        .ok()
                        .filter(serde_json::Value::is_object);
            }
            // Later argument deltas often omit identity. Keep emitting the known
            // id/name so live previews can bind one slot before ToolProposed.
            let id = id.or_else(|| partial.partial.id.clone());
            let name = name.or_else(|| partial.partial.name.clone());
            upsert_captured_tool_call(capture, index);
            RunEvent::ToolCallUpdated {
                index,
                id,
                name,
                arguments_delta: arguments,
            }
        }
        ModelEvent::ProviderContext {
            kind,
            position,
            data,
        } => {
            // Provider-native boundaries (for example Gemini thought signatures)
            // must not be collapsed into a single cancelled text block.
            capture.merge_output_text = false;
            capture.seal_next_text_part = true;
            capture.provider_context.push(ProviderContextBlock {
                identity: identity.clone(),
                kind: kind.clone(),
                position,
                data,
            });
            RunEvent::ProviderContextUpdated { kind }
        }
        ModelEvent::Usage(usage) => {
            // Providers may emit partial usage across multiple stream events
            // (for example Anthropic input/cache at message_start and later
            // output deltas). Merge within the turn instead of overwriting.
            capture.usage = capture.usage.saturating_add(&usage);
            RunEvent::UsageUpdated {
                usage: accumulated_usage.saturating_add(&capture.usage),
            }
        }
    }
}

#[cfg(test)]
#[path = "stream_capture_tests.rs"]
mod tests;
