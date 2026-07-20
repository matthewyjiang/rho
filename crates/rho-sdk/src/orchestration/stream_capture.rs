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
    /// Maps provider tool-call stream indexes onto `content` positions.
    tool_call_content_index: BTreeMap<usize, usize>,
    reasoning: String,
    reasoning_summary: String,
    provider_context: Vec<ProviderContextBlock>,
    partial_tool_calls: BTreeMap<usize, PartialToolCall>,
    usage: ModelUsage,
    failed_attempts: Vec<(ProviderErrorKind, ModelUsage)>,
}

impl StreamCapture {
    pub(super) fn usage(&self) -> &ModelUsage {
        &self.usage
    }

    pub(super) fn usage_mut(&mut self) -> &mut ModelUsage {
        &mut self.usage
    }

    pub(super) fn take_failed_attempts(&mut self) -> Vec<(ProviderErrorKind, ModelUsage)> {
        std::mem::take(&mut self.failed_attempts)
    }

    pub(super) fn push_failed_attempt(&mut self, kind: ProviderErrorKind, usage: ModelUsage) {
        self.failed_attempts.push((kind, usage));
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
            tool_calls: self.partial_tool_calls.into_values().collect(),
            usage: self.usage,
        })
    }
}

fn upsert_captured_tool_call(capture: &mut StreamCapture, index: usize) {
    let Some(partial) = capture.partial_tool_calls.get(&index) else {
        return;
    };
    let Some(id) = partial.id.as_ref().filter(|id| !id.is_empty()) else {
        return;
    };
    let Some(name) = partial.name.as_ref().filter(|name| !name.is_empty()) else {
        return;
    };
    let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&partial.arguments) else {
        return;
    };
    if !arguments.is_object() {
        return;
    }
    let call = ContentBlock::ToolCall(ToolCall {
        id: id.clone(),
        name: name.clone(),
        arguments,
    });
    if let Some(&content_index) = capture.tool_call_content_index.get(&index) {
        if let Some(slot) = capture.content.get_mut(content_index) {
            *slot = call;
            return;
        }
    }
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
                    capture.merge_output_text = true;
                    return RunEvent::AssistantTextDelta { text };
                };
                existing.push_str(&text);
            } else {
                capture.content.push(ContentBlock::Text(text.clone()));
                capture.merge_output_text = true;
            }
            RunEvent::AssistantTextDelta { text }
        }
        ModelEvent::ReasoningDelta(text) => {
            capture.merge_output_text = false;
            capture.reasoning.push_str(&text);
            RunEvent::ReasoningDelta { text }
        }
        ModelEvent::ReasoningSummaryDelta(text) => {
            capture.merge_output_text = false;
            capture.reasoning_summary.push_str(&text);
            RunEvent::ReasoningSummaryDelta { text }
        }
        ModelEvent::WebSearch(detail) => RunEvent::ProviderActivity {
            kind: crate::PROVIDER_ACTIVITY_WEB_SEARCH.into(),
            detail,
        },
        ModelEvent::ToolCallDelta {
            index,
            id,
            name,
            arguments,
        } => {
            capture.merge_output_text = false;
            let partial =
                capture
                    .partial_tool_calls
                    .entry(index)
                    .or_insert_with(|| PartialToolCall {
                        id: None,
                        name: None,
                        arguments: String::new(),
                    });
            if id.is_some() {
                partial.id.clone_from(&id);
            }
            if name.is_some() {
                partial.name.clone_from(&name);
            }
            partial.arguments.push_str(&arguments);
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
