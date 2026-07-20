use std::collections::HashMap;

use serde_json::json;

use crate::model::{
    handoff::prepare_assistant, ContentBlock, Message, ModelError, ModelEvent, ModelIdentity,
    ModelResponse, ModelUsage, ToolCall, ToolSpec,
};

use super::types::*;

pub const THOUGHT_SIGNATURE_CONTEXT: &str = "gemini-thought-signature";
pub const THOUGHT_PART_CONTEXT: &str = "gemini-thought-part";
pub const MISSING_FUNCTION_CALL_ID_CONTEXT: &str = "gemini-missing-function-call-id";

#[derive(Clone)]
struct CallInfo {
    name: String,
    has_provider_id: bool,
}

pub fn build_request(
    messages: &[Message],
    tools: &[ToolSpec],
    target: &ModelIdentity,
    reasoning: Option<ThinkingConfig>,
) -> Result<GenerateContentRequest, ModelError> {
    let system_text = messages
        .iter()
        .filter_map(|message| match message {
            Message::System(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut call_names = HashMap::new();
    let mut contents = Vec::new();
    for message in messages {
        let content = match message {
            Message::System(_) => continue,
            Message::User(blocks) => Content {
                role: Some(Role::User),
                parts: blocks_to_parts(blocks, &mut call_names),
            },
            Message::Assistant(blocks) => Content {
                role: Some(Role::Model),
                parts: blocks_to_parts(blocks, &mut call_names),
            },
            Message::EnrichedAssistant(message) => {
                let prepared = prepare_assistant((**message).clone(), target);
                let mut parts = blocks_to_parts(&prepared.content, &mut call_names);
                replay_provider_context(
                    &mut parts,
                    &prepared.replay_context,
                    target,
                    &mut call_names,
                )?;
                Content {
                    role: Some(Role::Model),
                    parts,
                }
            }
            Message::AbortedAssistant(message) => {
                let prepared = prepare_assistant(
                    crate::model::AssistantMessage {
                        content: aborted_content_blocks(message),
                        provenance: message.provenance.clone(),
                        reasoning_summary: message.reasoning_summary.clone(),
                        provider_context: message.provider_context.clone(),
                    },
                    target,
                );
                let mut parts = blocks_to_parts(&prepared.content, &mut call_names);
                replay_provider_context(
                    &mut parts,
                    &prepared.replay_context,
                    target,
                    &mut call_names,
                )?;
                // Append after replay so provider-context positions stay exact.
                parts.push(Part::text("[Operation aborted]"));
                Content {
                    role: Some(Role::Model),
                    parts,
                }
            }
            Message::ToolResult(result) => {
                let info = call_names.get(&result.id);
                let name = info
                    .map(|info| info.name.clone())
                    .unwrap_or_else(|| result.id.clone());
                Content {
                    role: Some(Role::User),
                    parts: vec![Part {
                        text: None,
                        inline_data: None,
                        function_call: None,
                        function_response: Some(FunctionResponse {
                            id: info
                                .is_none_or(|info| info.has_provider_id)
                                .then(|| result.id.clone()),
                            name,
                            response: json!({"output": result.content, "ok": result.ok}),
                        }),
                        thought: false,
                        thought_signature: None,
                    }],
                }
            }
        };
        if !content.parts.is_empty() {
            push_content(&mut contents, content);
        }
    }
    if contents.is_empty() {
        return Err(ModelError::InvalidResponse(
            "Gemini request has no conversation content".into(),
        ));
    }
    let tools = (!tools.is_empty()).then(|| {
        vec![Tool {
            function_declarations: tools
                .iter()
                .map(|tool| FunctionDeclaration {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters_json_schema: tool.input_schema.clone(),
                })
                .collect(),
        }]
    });
    Ok(GenerateContentRequest {
        system_instruction: (!system_text.is_empty()).then(|| Content {
            role: None,
            parts: vec![Part::text(system_text)],
        }),
        contents,
        tools,
        generation_config: reasoning.map(|thinking_config| GenerationConfig {
            thinking_config: Some(thinking_config),
        }),
    })
}

fn push_content(contents: &mut Vec<Content>, content: Content) {
    if let Some(previous) = contents
        .last_mut()
        .filter(|previous| previous.role == content.role)
    {
        previous.parts.extend(content.parts);
    } else {
        contents.push(content);
    }
}

fn replay_provider_context(
    parts: &mut Vec<Part>,
    contexts: &[crate::model::ProviderContextBlock],
    target: &ModelIdentity,
    call_names: &mut HashMap<String, CallInfo>,
) -> Result<(), ModelError> {
    let mut thought_parts = Vec::new();
    for (sequence, context) in contexts.iter().enumerate() {
        if !context.is_replayable_to(target) {
            continue;
        }
        let Some(position) = context.position else {
            continue;
        };
        match context.kind.as_str() {
            THOUGHT_SIGNATURE_CONTEXT => {
                let Some(signature) = context.data.as_str() else {
                    return Err(ModelError::InvalidResponse(
                        "invalid Gemini thought-signature context".into(),
                    ));
                };
                let Some(part) = parts.get_mut(position) else {
                    return Err(ModelError::InvalidResponse(
                        "Gemini thought-signature position is out of bounds".into(),
                    ));
                };
                part.thought_signature = Some(signature.to_string());
            }
            MISSING_FUNCTION_CALL_ID_CONTEXT => {
                let Some(part) = parts.get_mut(position) else {
                    return Err(ModelError::InvalidResponse(
                        "Gemini missing-call-ID position is out of bounds".into(),
                    ));
                };
                let Some(call) = part.function_call.as_mut() else {
                    return Err(ModelError::InvalidResponse(
                        "Gemini missing-call-ID context does not target a function call".into(),
                    ));
                };
                let Some(canonical_id) = call.id.take() else {
                    return Err(ModelError::InvalidResponse(
                        "Gemini function call has no canonical ID".into(),
                    ));
                };
                if let Some(info) = call_names.get_mut(&canonical_id) {
                    info.has_provider_id = false;
                }
            }
            THOUGHT_PART_CONTEXT => {
                let part =
                    serde_json::from_value::<Part>(context.data.clone()).map_err(|error| {
                        ModelError::InvalidResponse(format!(
                            "invalid Gemini thought-part context: {error}"
                        ))
                    })?;
                thought_parts.push((position, sequence, part));
            }
            _ => {}
        }
    }
    thought_parts.sort_by_key(|(position, sequence, _)| (*position, *sequence));
    for (position, _, part) in thought_parts.into_iter().rev() {
        parts.insert(position.min(parts.len()), part);
    }
    Ok(())
}

/// Rebuilds portable blocks for an aborted turn.
///
/// Cancellation history keeps completed text in `content` and tool-call fragments
/// in `tool_calls`. Gemini thought-signature positions refer to the original
/// collector content order, which includes tool-call blocks, so complete tool
/// fragments must be restored before replay.
fn aborted_content_blocks(message: &crate::model::AbortedAssistant) -> Vec<ContentBlock> {
    let mut blocks = message.content.clone();
    let mut existing_call_ids = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call.id.clone()),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    let mut tool_calls = message.tool_calls.clone();
    tool_calls.sort_by(|left, right| {
        left.id
            .as_deref()
            .unwrap_or_default()
            .cmp(right.id.as_deref().unwrap_or_default())
    });
    for call in tool_calls {
        let Some(id) = call.id.filter(|id| !id.is_empty()) else {
            continue;
        };
        if !existing_call_ids.insert(id.clone()) {
            continue;
        }
        let Some(name) = call.name.filter(|name| !name.is_empty()) else {
            continue;
        };
        let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments) else {
            continue;
        };
        if !arguments.is_object() {
            continue;
        }
        blocks.push(ContentBlock::ToolCall(ToolCall {
            id,
            name,
            arguments,
        }));
    }
    blocks
}

fn blocks_to_parts(
    blocks: &[ContentBlock],
    call_names: &mut HashMap<String, CallInfo>,
) -> Vec<Part> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => Part::text(text),
            ContentBlock::Image(image) => Part {
                text: None,
                inline_data: Some(InlineData {
                    mime_type: image.mime_type.clone(),
                    data: image.data.clone(),
                }),
                function_call: None,
                function_response: None,
                thought: false,
                thought_signature: None,
            },
            ContentBlock::ToolCall(call) => {
                call_names.insert(
                    call.id.clone(),
                    CallInfo {
                        name: call.name.clone(),
                        has_provider_id: true,
                    },
                );
                Part {
                    text: None,
                    inline_data: None,
                    function_call: Some(FunctionCall {
                        id: Some(call.id.clone()),
                        name: call.name.clone(),
                        args: call.arguments.clone(),
                    }),
                    function_response: None,
                    thought: false,
                    thought_signature: None,
                }
            }
        })
        .collect()
}

#[derive(Default)]
pub struct ResponseCollector {
    content: Vec<ContentBlock>,
    next_call: usize,
    can_merge_output_text: bool,
    has_emitted_output: bool,
    reported_usage_metadata: UsageMetadata,
    reported_usage: ModelUsage,
}

impl ResponseCollector {
    pub fn apply(
        &mut self,
        response: GenerateContentResponse,
        mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    ) -> Result<(), ModelError> {
        let usage_delta = response.usage_metadata.map(|usage| {
            let usage = merge_cumulative_usage(&self.reported_usage_metadata, usage);
            let current = usage_from_metadata(usage.clone());
            self.reported_usage_metadata = usage;
            let delta = usage_delta(&current, &self.reported_usage);
            self.reported_usage = current;
            delta
        });
        if let Some(reason) = response
            .prompt_feedback
            .and_then(|feedback| feedback.block_reason)
        {
            emit_usage(&mut on_event, usage_delta.as_ref())?;
            return Err(ModelError::InvalidResponse(format!(
                "Gemini blocked the prompt: {reason}"
            )));
        }
        for candidate in response.candidates {
            if let Some(reason) = candidate.finish_reason {
                if !reason.is_success() {
                    emit_usage(&mut on_event, usage_delta.as_ref())?;
                    return Err(ModelError::InvalidResponse(format!(
                        "Gemini stopped with {reason:?}: {}",
                        candidate.finish_message.unwrap_or_default()
                    )));
                }
            }
            let Some(content) = candidate.content else {
                continue;
            };
            for part in content.parts {
                let has_text = part.text.is_some();
                let has_signature = part.thought_signature.is_some();
                self.has_emitted_output |= has_text || part.function_call.is_some();
                // Gemini forbids merging a signed part with an unsigned neighbor.
                let merges_with_previous_text = !part.thought
                    && part.function_call.is_none()
                    && part.text.is_some()
                    && !has_signature
                    && self.can_merge_output_text;
                let portable_position = if merges_with_previous_text {
                    self.content.len().saturating_sub(1)
                } else {
                    self.content.len()
                };
                let is_portable =
                    !part.thought && (part.text.is_some() || part.function_call.is_some());
                if part.thought {
                    emit(
                        &mut on_event,
                        ModelEvent::ProviderContext {
                            kind: THOUGHT_PART_CONTEXT.into(),
                            position: Some(self.content.len()),
                            data: serde_json::to_value(&part).map_err(|error| {
                                ModelError::InvalidResponse(format!(
                                    "failed to preserve Gemini thought part: {error}"
                                ))
                            })?,
                        },
                    )?;
                } else if let Some(signature) = &part.thought_signature {
                    let (kind, data) = if is_portable {
                        (THOUGHT_SIGNATURE_CONTEXT, json!(signature))
                    } else {
                        (
                            THOUGHT_PART_CONTEXT,
                            serde_json::to_value(&part).map_err(|error| {
                                ModelError::InvalidResponse(format!(
                                    "failed to preserve Gemini signature-only part: {error}"
                                ))
                            })?,
                        )
                    };
                    emit(
                        &mut on_event,
                        ModelEvent::ProviderContext {
                            kind: kind.into(),
                            position: Some(if is_portable {
                                portable_position
                            } else {
                                self.content.len()
                            }),
                            data,
                        },
                    )?;
                }
                if let Some(text) = part.text {
                    if part.thought {
                        self.can_merge_output_text = false;
                        emit(&mut on_event, ModelEvent::ReasoningSummaryDelta(text))?;
                    } else {
                        emit(&mut on_event, ModelEvent::OutputDelta(text.clone()))?;
                        if merges_with_previous_text {
                            let Some(ContentBlock::Text(previous)) = self.content.last_mut() else {
                                return Err(ModelError::InvalidResponse(
                                    "Gemini text merge state is inconsistent".into(),
                                ));
                            };
                            previous.push_str(&text);
                        } else {
                            self.content.push(ContentBlock::Text(text));
                        }
                        // Signed text must remain a standalone native part.
                        self.can_merge_output_text = !has_signature;
                    }
                }
                if let Some(call) = part.function_call {
                    self.can_merge_output_text = false;
                    let index = self.next_call;
                    self.next_call += 1;
                    let id = if let Some(id) = call.id {
                        id
                    } else {
                        let id = format!("gemini-call-{:032x}", rand::random::<u128>());
                        emit(
                            &mut on_event,
                            ModelEvent::ProviderContext {
                                kind: MISSING_FUNCTION_CALL_ID_CONTEXT.into(),
                                position: Some(self.content.len()),
                                data: serde_json::Value::Null,
                            },
                        )?;
                        id
                    };
                    emit(
                        &mut on_event,
                        ModelEvent::ToolCallDelta {
                            index,
                            id: Some(id.clone()),
                            name: Some(call.name.clone()),
                            arguments: call.args.to_string(),
                        },
                    )?;
                    self.content.push(ContentBlock::ToolCall(ToolCall {
                        id,
                        name: call.name,
                        arguments: call.args,
                    }));
                } else if !has_text {
                    self.can_merge_output_text = false;
                }
            }
        }
        emit_usage(&mut on_event, usage_delta.as_ref())?;
        Ok(())
    }

    pub fn has_emitted_output(&self) -> bool {
        self.has_emitted_output
    }

    pub fn finish(self) -> Result<ModelResponse, ModelError> {
        if self.content.is_empty() {
            return Err(ModelError::InvalidResponse(
                "Gemini returned no content".into(),
            ));
        }
        Ok(ModelResponse::Assistant(self.content))
    }
}

fn usage_from_metadata(usage: UsageMetadata) -> ModelUsage {
    let cached = usage.cached_content_token_count;
    ModelUsage {
        input_tokens: usage
            .prompt_token_count
            .map(|total| total.saturating_sub(cached.unwrap_or_default())),
        output_tokens: match (usage.candidates_token_count, usage.thoughts_token_count) {
            (None, None) => None,
            (answer, thoughts) => Some(
                answer
                    .unwrap_or_default()
                    .saturating_add(thoughts.unwrap_or_default()),
            ),
        },
        cache_read_tokens: cached,
        total_tokens: usage.total_token_count,
        ..ModelUsage::default()
    }
}

fn merge_cumulative_usage(previous: &UsageMetadata, observed: UsageMetadata) -> UsageMetadata {
    UsageMetadata {
        prompt_token_count: observed.prompt_token_count.or(previous.prompt_token_count),
        candidates_token_count: observed
            .candidates_token_count
            .or(previous.candidates_token_count),
        total_token_count: observed.total_token_count.or(previous.total_token_count),
        cached_content_token_count: observed
            .cached_content_token_count
            .or(previous.cached_content_token_count),
        thoughts_token_count: observed
            .thoughts_token_count
            .or(previous.thoughts_token_count),
    }
}

fn usage_delta(current: &ModelUsage, previous: &ModelUsage) -> ModelUsage {
    fn delta(current: Option<u64>, previous: Option<u64>) -> Option<u64> {
        current.map(|current| current.saturating_sub(previous.unwrap_or_default()))
    }

    ModelUsage {
        input_tokens: delta(current.input_tokens, previous.input_tokens),
        output_tokens: delta(current.output_tokens, previous.output_tokens),
        cache_read_tokens: delta(current.cache_read_tokens, previous.cache_read_tokens),
        cache_write_tokens: delta(current.cache_write_tokens, previous.cache_write_tokens),
        total_tokens: delta(current.total_tokens, previous.total_tokens),
        context_window: current.context_window,
        cost_usd_micros: delta(current.cost_usd_micros, previous.cost_usd_micros),
    }
}

fn emit_usage(
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    usage: Option<&ModelUsage>,
) -> Result<(), ModelError> {
    if let Some(usage) = usage {
        emit(on_event, ModelEvent::Usage(usage.clone()))?;
    }
    Ok(())
}

fn emit(
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    event: ModelEvent,
) -> Result<(), ModelError> {
    if let Some(callback) = on_event.as_deref_mut() {
        callback(event)?;
    }
    Ok(())
}
