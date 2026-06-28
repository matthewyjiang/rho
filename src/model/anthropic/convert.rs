use crate::model::{ContentBlock, Message, ModelError, ModelResponse, ModelUsage};
use crate::tool::{ToolCall, ToolSpec};

use super::types::{
    AnthropicContentBlock, AnthropicImageSource, AnthropicMessage, AnthropicResponse,
    AnthropicRole, AnthropicTool, AnthropicUsage,
};

fn resolved_cache_read_tokens(usage: &AnthropicUsage) -> Option<u64> {
    usage.cache_read_input_tokens
}

fn resolved_cache_write_tokens(usage: &AnthropicUsage) -> Option<u64> {
    usage.cache_creation_input_tokens.or_else(|| {
        usage.cache_creation.as_ref().and_then(|cache| {
            cache
                .ephemeral_1h_input_tokens
                .or(cache.ephemeral_5m_input_tokens)
        })
    })
}

pub(super) fn split_system_and_messages(
    messages: Vec<Message>,
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

pub(super) fn to_anthropic_tool(tool: ToolSpec) -> AnthropicTool {
    AnthropicTool {
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
        cache_control: None,
    }
}

pub(super) fn convert_anthropic_response(
    response: AnthropicResponse,
) -> Result<ModelResponse, ModelError> {
    let _usage = response.usage.map(usage_to_model_usage);
    convert_content_blocks(response.content)
}

pub(super) fn convert_content_blocks(
    content: Vec<AnthropicContentBlock>,
) -> Result<ModelResponse, ModelError> {
    let mut blocks = Vec::new();
    for block in content {
        match block {
            AnthropicContentBlock::Text { text, .. } if !text.is_empty() => {
                blocks.push(ContentBlock::Text(text));
            }
            AnthropicContentBlock::Text { text: _, .. } => {}
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

pub(super) fn usage_to_model_usage(usage: AnthropicUsage) -> ModelUsage {
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
    use crate::model::{anthropic::types::AnthropicCacheCreation, ImageContent};
    use crate::tool::ToolResult;

    #[test]
    fn converts_messages_and_tools_to_anthropic_shape() {
        let (system, messages) = split_system_and_messages(vec![
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
        ])
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
        let (_system, messages) = split_system_and_messages(vec![Message::User(vec![
            ContentBlock::Text("look".into()),
            ContentBlock::Image(ImageContent {
                data: "aW1n".into(),
                mime_type: "image/png".into(),
            }),
        ])])
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
        let (_system, messages) =
            split_system_and_messages(vec![Message::ToolResult(ToolResult {
                id: "toolu_1".into(),
                ok: false,
                content: "failed".into(),
            })])
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
        let (_system, messages) = split_system_and_messages(vec![
            Message::user_text("one"),
            Message::user_text("two"),
            Message::assistant_text("three"),
        ])
        .unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, AnthropicRole::User);
        assert_eq!(messages[0].content.len(), 2);
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
            cache_creation: None,
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
                ephemeral_5m_input_tokens: None,
            }),
        });

        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.cache_write_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(19));
    }
}
