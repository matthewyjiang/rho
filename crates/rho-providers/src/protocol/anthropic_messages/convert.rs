use crate::{
    model::handoff::prepare_assistant,
    provider_backend::{
        ContentBlock, Message, ModelError, ModelResponse, ModelUsage, ToolCall, ToolSpec,
    },
};

use super::types::{
    AnthropicContentBlock, AnthropicImageSource, AnthropicMessage, AnthropicResponse,
    AnthropicRole, AnthropicTool, AnthropicUsage,
};

fn resolved_cache_read_tokens(usage: &AnthropicUsage) -> Option<u64> {
    usage.cache_read_input_tokens
}

fn resolved_cache_write_tokens(usage: &AnthropicUsage) -> Option<u64> {
    usage.cache_creation_input_tokens.or_else(|| {
        let cache = usage.cache_creation.as_ref()?;
        match (
            cache.ephemeral_1h_input_tokens,
            cache.ephemeral_5m_input_tokens,
        ) {
            (None, None) => None,
            (one_hour, five_minutes) => Some(
                one_hour
                    .unwrap_or_default()
                    .saturating_add(five_minutes.unwrap_or_default()),
            ),
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderContextReplay {
    Enabled,
    Disabled,
}

pub(crate) fn split_system_and_messages(
    messages: Vec<Message>,
    target: &crate::model::ModelIdentity,
    provider_context_replay: ProviderContextReplay,
) -> Result<(Option<String>, Vec<AnthropicMessage>), ModelError> {
    let mut system = Vec::new();
    let mut converted = Vec::new();
    for message in messages {
        match message {
            Message::System(content) => system.push(content),
            Message::User(blocks) => push_message(
                &mut converted,
                AnthropicRole::User,
                blocks.into_iter().map(user_block).collect(),
            ),
            Message::Assistant(blocks) => push_message(
                &mut converted,
                AnthropicRole::Assistant,
                blocks.into_iter().map(assistant_block).collect(),
            ),
            Message::EnrichedAssistant(message) => {
                let mut message = *message;
                if provider_context_replay == ProviderContextReplay::Disabled {
                    message.retain_portable_context();
                }
                let prepared = prepare_assistant(message, target);
                let mut content = prepared
                    .content
                    .into_iter()
                    .map(assistant_block)
                    .collect::<Vec<_>>();
                for block in prepared.replay_context {
                    if provider_context_replay == ProviderContextReplay::Disabled
                        || block.kind != "anthropic_content_block"
                    {
                        continue;
                    }
                    if let Ok(block_data) = serde_json::from_value(block.data) {
                        content.insert(
                            block.position.unwrap_or(content.len()).min(content.len()),
                            block_data,
                        );
                    }
                }
                push_message(&mut converted, AnthropicRole::Assistant, content);
            }
            Message::AbortedAssistant(message) => {
                let content = message
                    .content
                    .into_iter()
                    .map(|block| match block {
                        ContentBlock::ToolCall(call) => ContentBlock::Text(render_tool_call(&call)),
                        other => other,
                    })
                    .collect::<Vec<_>>();
                let mut enriched = crate::model::AssistantMessage {
                    content,
                    provenance: message.provenance,
                    reasoning_summary: message.reasoning_summary,
                    provider_context: message.provider_context,
                };
                enriched
                    .content
                    .push(ContentBlock::Text("[Operation aborted]".into()));
                if provider_context_replay == ProviderContextReplay::Disabled {
                    enriched.retain_portable_context();
                }
                let prepared = prepare_assistant(enriched, target);
                let mut content = prepared
                    .content
                    .into_iter()
                    .map(assistant_block)
                    .collect::<Vec<_>>();
                for block in prepared.replay_context {
                    if provider_context_replay == ProviderContextReplay::Enabled
                        && block.kind == "anthropic_content_block"
                    {
                        if let Ok(block_data) = serde_json::from_value(block.data) {
                            content.insert(
                                block.position.unwrap_or(content.len()).min(content.len()),
                                block_data,
                            );
                        }
                    }
                }
                push_message(&mut converted, AnthropicRole::Assistant, content);
            }
            Message::ToolResult(result) => push_message(
                &mut converted,
                AnthropicRole::User,
                vec![AnthropicContentBlock::ToolResult {
                    tool_use_id: result.id,
                    content: result.content,
                    is_error: !result.ok,
                    cache_control: None,
                }],
            ),
        }
    }
    let system = (!system.is_empty()).then(|| system.join("\n\n"));
    Ok((system, converted))
}

fn push_message(
    messages: &mut Vec<AnthropicMessage>,
    role: AnthropicRole,
    mut content: Vec<AnthropicContentBlock>,
) {
    if let Some(previous) = messages.last_mut().filter(|message| message.role == role) {
        previous.content.append(&mut content);
    } else {
        messages.push(AnthropicMessage { role, content });
    }
}

fn user_block(block: ContentBlock) -> AnthropicContentBlock {
    match block {
        ContentBlock::Text(text) => AnthropicContentBlock::Text {
            text,
            cache_control: None,
        },
        ContentBlock::Image(image) => AnthropicContentBlock::Image {
            source: AnthropicImageSource {
                kind: "base64".into(),
                media_type: image.mime_type,
                data: image.data,
            },
        },
        ContentBlock::ToolCall(call) => AnthropicContentBlock::Text {
            text: render_tool_call(&call),
            cache_control: None,
        },
    }
}

fn assistant_block(block: ContentBlock) -> AnthropicContentBlock {
    match block {
        ContentBlock::Text(text) => AnthropicContentBlock::Text {
            text,
            cache_control: None,
        },
        ContentBlock::Image(image) => AnthropicContentBlock::Text {
            text: format!("[image: {}]", image.mime_type),
            cache_control: None,
        },
        ContentBlock::ToolCall(call) => AnthropicContentBlock::ToolUse {
            id: call.id,
            name: call.name,
            input: call.arguments,
        },
    }
}

fn render_tool_call(call: &ToolCall) -> String {
    let arguments = serde_json::to_string_pretty(&call.arguments)
        .unwrap_or_else(|_| call.arguments.to_string());
    format!("Tool call: {}\n{}", call.name, arguments)
}

pub(crate) fn to_anthropic_tool(tool: ToolSpec) -> AnthropicTool {
    let mut input_schema = tool.input_schema;
    if let Some(schema) = input_schema.as_object_mut() {
        schema.remove("oneOf");
        schema.remove("allOf");
        schema.remove("anyOf");
    }
    AnthropicTool {
        name: tool.name,
        description: tool.description,
        input_schema,
        cache_control: None,
    }
}

pub(crate) fn convert_anthropic_response(
    response: AnthropicResponse,
) -> Result<ModelResponse, ModelError> {
    let _usage = response.usage.map(usage_to_model_usage);
    convert_content_blocks(response.content)
}

pub(crate) fn convert_content_blocks(
    content: Vec<AnthropicContentBlock>,
) -> Result<ModelResponse, ModelError> {
    let mut blocks = Vec::new();
    for block in content {
        match block {
            AnthropicContentBlock::Text { text, .. } if !text.is_empty() => {
                blocks.push(ContentBlock::Text(text));
            }
            AnthropicContentBlock::Text { .. } => {}
            AnthropicContentBlock::Thinking { .. }
            | AnthropicContentBlock::RedactedThinking { .. } => {}
            AnthropicContentBlock::Image { .. } => {}
            AnthropicContentBlock::ToolUse { id, name, input } => {
                blocks.push(ContentBlock::ToolCall(ToolCall {
                    id,
                    name,
                    arguments: input,
                }));
            }
            AnthropicContentBlock::ToolResult { .. } => {
                return Err(ModelError::InvalidResponse(
                    "assistant response contained tool_result block".into(),
                ));
            }
        }
    }
    if blocks.is_empty() {
        Err(ModelError::InvalidResponse(
            "assistant message had no content or tool calls".into(),
        ))
    } else {
        Ok(ModelResponse::Assistant(blocks))
    }
}

pub(crate) fn usage_to_model_usage(usage: AnthropicUsage) -> ModelUsage {
    let cache_read_tokens = resolved_cache_read_tokens(&usage);
    let cache_write_tokens = resolved_cache_write_tokens(&usage);
    let total_tokens = usage
        .input_tokens
        .unwrap_or_default()
        .saturating_add(cache_read_tokens.unwrap_or_default())
        .saturating_add(cache_write_tokens.unwrap_or_default())
        .saturating_add(usage.output_tokens.unwrap_or_default());
    let has_total = usage.input_tokens.is_some()
        || cache_read_tokens.is_some()
        || cache_write_tokens.is_some()
        || usage.output_tokens.is_some();
    ModelUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens: has_total.then_some(total_tokens),
        context_window: None,
        cost_usd_micros: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        protocol::anthropic_messages::types::AnthropicCacheCreation,
        provider_backend::{ImageContent, ToolResult},
    };

    fn target() -> crate::model::ModelIdentity {
        crate::model::ModelIdentity::new("anthropic", "anthropic-messages", "claude-test")
    }

    #[test]
    fn converts_messages_and_tools_to_anthropic_shape() {
        let (system, messages) = split_system_and_messages(
            vec![
                Message::System("first".into()),
                Message::System("second".into()),
                Message::User(vec![ContentBlock::Text("hello".into())]),
                Message::Assistant(vec![
                    ContentBlock::Text("I'll check".into()),
                    ContentBlock::ToolCall(ToolCall {
                        id: "toolu_1".into(),
                        name: "bash".into(),
                        arguments: json!({"command":"pwd"}),
                    }),
                ]),
                Message::ToolResult(ToolResult {
                    id: "toolu_1".into(),
                    ok: true,
                    content: "/repo".into(),
                }),
            ],
            &target(),
            ProviderContextReplay::Enabled,
        )
        .unwrap();

        assert_eq!(system, Some("first\n\nsecond".into()));
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, AnthropicRole::User);
        assert_eq!(messages[1].role, AnthropicRole::Assistant);
        assert_eq!(messages[2].role, AnthropicRole::User);
        assert_eq!(
            messages[1].content[1],
            AnthropicContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "bash".into(),
                input: json!({"command":"pwd"}),
            }
        );
        assert_eq!(
            messages[2].content[0],
            AnthropicContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: "/repo".into(),
                is_error: false,
                cache_control: None,
            }
        );
    }

    #[test]
    fn converts_user_images_to_anthropic_shape() {
        let (_system, messages) = split_system_and_messages(
            vec![Message::User(vec![
                ContentBlock::Text("look".into()),
                ContentBlock::Image(ImageContent {
                    data: "aW1n".into(),
                    mime_type: "image/png".into(),
                }),
            ])],
            &target(),
            ProviderContextReplay::Enabled,
        )
        .unwrap();

        assert_eq!(messages[0].role, AnthropicRole::User);
        assert_eq!(
            messages[0].content[1],
            AnthropicContentBlock::Image {
                source: AnthropicImageSource {
                    kind: "base64".into(),
                    media_type: "image/png".into(),
                    data: "aW1n".into(),
                },
            }
        );
    }

    #[test]
    fn marks_failed_tool_results_as_errors() {
        let (_system, messages) = split_system_and_messages(
            vec![Message::ToolResult(ToolResult {
                id: "toolu_1".into(),
                ok: false,
                content: "failed".into(),
            })],
            &target(),
            ProviderContextReplay::Enabled,
        )
        .unwrap();

        assert_eq!(
            messages[0].content[0],
            AnthropicContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: "failed".into(),
                is_error: true,
                cache_control: None,
            }
        );
    }

    #[test]
    fn merges_consecutive_same_role_messages() {
        let (_system, messages) = split_system_and_messages(
            vec![
                Message::user_text("one"),
                Message::user_text("two"),
                Message::assistant_text("three"),
            ],
            &target(),
            ProviderContextReplay::Enabled,
        )
        .unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, AnthropicRole::User);
        assert_eq!(messages[0].content.len(), 2);
    }

    #[test]
    fn converts_foreign_summary_and_omits_foreign_provider_context() {
        let source =
            crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
        let (_, messages) = split_system_and_messages(
            vec![Message::assistant(crate::model::AssistantMessage {
                content: vec![ContentBlock::Text("answer".into())],
                provenance: Some(source.clone()),
                reasoning_summary: Some("verified it".into()),
                provider_context: vec![crate::model::ProviderContextBlock {
                    identity: source,
                    kind: "openai_response_output_item".into(),
                    position: None,
                    data: json!({"type": "reasoning", "encrypted_content": "signed"}),
                }],
            })],
            &target(),
            ProviderContextReplay::Enabled,
        )
        .unwrap();

        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0].content.as_slice(),
            [AnthropicContentBlock::Text { text: answer, .. }, AnthropicContentBlock::Text { text: summary, .. }]
                if answer == "answer" && summary.contains("<reasoning_summary>") && summary.contains("verified it")
        ));
    }

    #[test]
    fn disabled_replay_preserves_foreign_portable_fallback() {
        let source =
            crate::model::ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
        let message = crate::model::AssistantMessage {
            provenance: Some(source.clone()),
            provider_context: vec![crate::model::ProviderContextBlock {
                identity: source,
                kind: "openai_response_output_item".into(),
                position: Some(0),
                data: json!({"type": "compaction", "encrypted_content": "signed"}),
            }],
            ..crate::model::AssistantMessage::default()
        }
        .with_portable_fallback("portable notice");

        let (_, messages) = split_system_and_messages(
            vec![Message::assistant(message)],
            &target(),
            ProviderContextReplay::Disabled,
        )
        .unwrap();

        assert!(matches!(
            messages[0].content.as_slice(),
            [AnthropicContentBlock::Text { text, .. }] if text == "portable notice"
        ));
    }

    #[test]
    fn exact_anthropic_handoff_replays_signed_thinking_in_original_position() {
        let target = target();
        let (_, messages) = split_system_and_messages(
            vec![Message::assistant(crate::model::AssistantMessage {
                content: vec![ContentBlock::Text("answer".into())],
                provenance: Some(target.clone()),
                reasoning_summary: None,
                provider_context: vec![crate::model::ProviderContextBlock {
                    identity: target.clone(),
                    kind: "anthropic_content_block".into(),
                    position: Some(0),
                    data: json!({
                        "type": "thinking",
                        "thinking": "private",
                        "signature": "signed"
                    }),
                }],
            })],
            &target,
            ProviderContextReplay::Enabled,
        )
        .unwrap();

        assert!(matches!(
            messages[0].content.as_slice(),
            [AnthropicContentBlock::Thinking { thinking, signature }, AnthropicContentBlock::Text { text, .. }]
                if thinking == "private" && signature == "signed" && text == "answer"
        ));
    }

    #[test]
    fn exact_anthropic_handoff_omits_thinking_when_reasoning_is_disabled() {
        let target = target();
        let (_, messages) = split_system_and_messages(
            vec![Message::assistant(crate::model::AssistantMessage {
                content: vec![ContentBlock::Text("answer".into())],
                provenance: Some(target.clone()),
                reasoning_summary: Some("safe summary".into()),
                provider_context: vec![crate::model::ProviderContextBlock {
                    identity: target.clone(),
                    kind: "anthropic_content_block".into(),
                    position: Some(0),
                    data: json!({
                        "type": "thinking",
                        "thinking": "private",
                        "signature": "signed"
                    }),
                }],
            })],
            &target,
            ProviderContextReplay::Disabled,
        )
        .unwrap();

        assert!(matches!(
            messages[0].content.as_slice(),
            [AnthropicContentBlock::Text { text, .. }, AnthropicContentBlock::Text { text: summary, .. }]
                if text == "answer" && summary.contains("safe summary")
        ));
    }

    #[test]
    fn converts_response_text_and_tool_use() {
        let response = AnthropicResponse {
            content: vec![
                AnthropicContentBlock::Text {
                    text: "hi".into(),
                    cache_control: None,
                },
                AnthropicContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "bash".into(),
                    input: json!({"command":"pwd"}),
                },
            ],
            usage: None,
        };

        let ModelResponse::Assistant(blocks) = convert_anthropic_response(response).unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], ContentBlock::Text(_)));
        assert!(matches!(blocks[1], ContentBlock::ToolCall(_)));
    }

    #[test]
    fn maps_usage() {
        let usage = usage_to_model_usage(AnthropicUsage {
            input_tokens: Some(10),
            output_tokens: Some(4),
            cache_read_input_tokens: Some(3),
            cache_creation_input_tokens: Some(2),
            cache_creation: Some(AnthropicCacheCreation {
                ephemeral_1h_input_tokens: Some(2),
                ephemeral_5m_input_tokens: Some(5),
            }),
        });

        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.cache_write_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(19));
    }

    #[test]
    fn maps_nested_cache_creation_usage() {
        let usage = usage_to_model_usage(AnthropicUsage {
            input_tokens: Some(10),
            output_tokens: Some(4),
            cache_read_input_tokens: Some(3),
            cache_creation_input_tokens: None,
            cache_creation: Some(AnthropicCacheCreation {
                ephemeral_1h_input_tokens: Some(2),
                ephemeral_5m_input_tokens: Some(5),
            }),
        });

        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.cache_write_tokens, Some(7));
        assert_eq!(usage.total_tokens, Some(24));
    }
}
