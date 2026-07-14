use std::collections::BTreeMap;

use crate::model::{
    AbortedAssistant, ContentBlock, Message, ModelEvent, ModelIdentity, ModelUsage,
    PartialToolCall, ProviderContextBlock,
};

#[derive(Default)]
pub(super) struct PartialAssistant {
    pub(super) text: String,
    pub(super) reasoning: String,
    pub(super) reasoning_summary: String,
    pub(super) tool_calls: BTreeMap<usize, PartialToolCall>,
    pub(super) provider_context: Vec<ProviderContextBlock>,
    pub(super) usage: ModelUsage,
}

impl PartialAssistant {
    pub(super) fn record(&mut self, event: &ModelEvent) {
        match event {
            ModelEvent::OutputDelta(delta) => self.text.push_str(delta),
            ModelEvent::ReasoningSummaryDelta(delta) => self.reasoning_summary.push_str(delta),
            ModelEvent::ReasoningDelta(delta) => self.reasoning.push_str(delta),
            ModelEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments,
            } => {
                let call = self
                    .tool_calls
                    .entry(*index)
                    .or_insert_with(|| PartialToolCall {
                        id: None,
                        name: None,
                        arguments: String::new(),
                    });
                if id.is_some() {
                    call.id.clone_from(id);
                }
                if name.is_some() {
                    call.name.clone_from(name);
                }
                call.arguments.push_str(arguments);
            }
            ModelEvent::ProviderContext {
                kind,
                position,
                data,
            } => {
                self.provider_context.push(ProviderContextBlock {
                    identity: ModelIdentity::new("unknown", "unknown", "unknown"),
                    kind: kind.clone(),
                    position: *position,
                    data: data.clone(),
                });
            }
            ModelEvent::Usage(usage) => merge_model_usage(&mut self.usage, usage),
            ModelEvent::WebSearch(_) => {}
        }
    }

    pub(super) fn into_message(self, identity: Option<ModelIdentity>) -> Message {
        let content = if self.text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text(self.text)]
        };
        let provider_context = self
            .provider_context
            .into_iter()
            .map(|mut block| {
                if let Some(identity) = &identity {
                    block.identity = identity.clone();
                }
                block
            })
            .collect();
        Message::AbortedAssistant(Box::new(AbortedAssistant {
            content,
            reasoning: self.reasoning,
            provenance: identity.clone(),
            reasoning_summary: (!self.reasoning_summary.is_empty()
                && identity
                    .as_ref()
                    .is_some_and(|identity| identity.api == "openai-responses"))
            .then_some(self.reasoning_summary),
            provider_context,
            tool_calls: self.tool_calls.into_values().collect(),
            usage: self.usage,
        }))
    }
}

fn merge_model_usage(total: &mut ModelUsage, update: &ModelUsage) {
    fn merge(target: &mut Option<u64>, update: Option<u64>) {
        if let Some(update) = update {
            *target = Some(target.unwrap_or_default().saturating_add(update));
        }
    }
    merge(&mut total.input_tokens, update.input_tokens);
    merge(&mut total.output_tokens, update.output_tokens);
    merge(&mut total.cache_read_tokens, update.cache_read_tokens);
    merge(&mut total.cache_write_tokens, update.cache_write_tokens);
    merge(&mut total.total_tokens, update.total_tokens);
    merge(&mut total.cost_usd_micros, update.cost_usd_micros);
    if update.context_window.is_some() {
        total.context_window = update.context_window;
    }
}
