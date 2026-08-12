use std::collections::BTreeMap;

use prost::Message;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::model::{
    ContentBlock, Message as ChatMessage, ModelError, ModelRequest, ToolCall, ToolResult,
};

use super::connect::encode_client_message;
use super::effort::CursorEffort;
use super::fast::{catalog_model_id, wire_model_id, CursorSpeed};
use super::ids::{deterministic_uuid, random_uuid, to_hex};
use super::mcp::mcp_tool_definitions;
use super::proto::{
    agent_client_message, conversation_action, conversation_step, conversation_turn_structure,
    AgentClientMessage, AgentConversationTurnStructure, AgentRunRequest, AssistantMessage,
    ConversationAction, ConversationStateStructure, ConversationStep, ConversationTurnStructure,
    McpToolDefinition, ModelDetails, RequestedModel, UserMessage, UserMessageAction,
};

/// Fresh-user-turn text when Rho history ends on assistant output, not a tool result.
///
/// Tool follow-ups do not use this: trailing tool results become the action so
/// Cursor cannot `ResumeAction` a stream that died mid-MCP call.
const CONTINUE_ACTION: &str = "Continue.";

#[derive(Clone, Debug, Default)]
pub(crate) struct BlobStore {
    blobs: BTreeMap<String, Vec<u8>>,
}

impl BlobStore {
    pub(crate) fn insert(&mut self, blob_id: &[u8], data: Vec<u8>) {
        self.blobs.insert(to_hex(blob_id), data);
    }

    pub(crate) fn get(&self, blob_id: &[u8]) -> Option<&[u8]> {
        self.blobs.get(&to_hex(blob_id)).map(Vec::as_slice)
    }

    fn store(&mut self, data: &[u8]) -> Vec<u8> {
        let blob_id = Sha256::digest(data).to_vec();
        self.insert(&blob_id, data.to_vec());
        blob_id
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CursorTurn {
    pub request_bytes: Vec<u8>,
    pub blob_store: BlobStore,
    pub mcp_tools: Vec<McpToolDefinition>,
    pub cloud_rule: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryKind {
    User,
    Assistant,
    Tool,
}

struct HistoryEntry {
    kind: HistoryKind,
    text: String,
}

struct ParsedMessages {
    system_prompts: Vec<String>,
    user_text: String,
    history: Vec<HistoryEntry>,
}

pub(crate) fn build_cursor_turn(
    model: &str,
    request: ModelRequest<'_>,
    speed: CursorSpeed,
    effort: CursorEffort,
) -> Result<CursorTurn, ModelError> {
    let parsed = parse_messages(request.messages);
    if parsed.user_text.is_empty() {
        return Err(ModelError::InvalidResponse(
            "Cursor request has no user message".into(),
        ));
    }
    // New id every Run. Reusing prompt_cache_key made ResumeAction replay the
    // MCP call we just tore the stream down on.
    let conversation_id = random_uuid();
    let mut blob_store = BlobStore::default();
    let request_bytes = encode_run_request(
        model,
        speed,
        effort,
        &parsed,
        &conversation_id,
        &mut blob_store,
    )?;
    let cloud_rule = parsed
        .system_prompts
        .iter()
        .map(String::as_str)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(CursorTurn {
        request_bytes,
        blob_store,
        mcp_tools: mcp_tool_definitions(request.tools),
        cloud_rule: (!cloud_rule.is_empty()).then_some(cloud_rule),
    })
}

fn encode_run_request(
    model: &str,
    speed: CursorSpeed,
    effort: CursorEffort,
    parsed: &ParsedMessages,
    conversation_id: &str,
    blob_store: &mut BlobStore,
) -> Result<Vec<u8>, ModelError> {
    let prompts = if parsed.system_prompts.is_empty() {
        vec!["You are a helpful assistant.".to_string()]
    } else {
        parsed.system_prompts.clone()
    };
    // AgentService reads both OpenAI-style prompt blobs and protobuf turn blobs.
    let system_blob_ids = prompts
        .iter()
        .map(|content| {
            blob_store.store(
                json!({ "role": "system", "content": content })
                    .to_string()
                    .as_bytes(),
            )
        })
        .collect::<Vec<_>>();
    let mut root_prompt_messages_json = system_blob_ids;
    for entry in &parsed.history {
        root_prompt_messages_json.push(store_prompt_blob(blob_store, entry));
    }

    let mut turn_blob_ids = Vec::new();
    let mut current_turn: Option<(Vec<u8>, Vec<Vec<u8>>)> = None;
    let flush_turn = |blob_store: &mut BlobStore,
                      turn_blob_ids: &mut Vec<Vec<u8>>,
                      current_turn: &mut Option<(Vec<u8>, Vec<Vec<u8>>)>| {
        let Some((user_message, steps)) = current_turn.take() else {
            return;
        };
        let turn = ConversationTurnStructure {
            turn: Some(conversation_turn_structure::Turn::AgentConversationTurn(
                AgentConversationTurnStructure {
                    user_message,
                    steps,
                },
            )),
        };
        turn_blob_ids.push(blob_store.store(&turn.encode_to_vec()));
    };
    for entry in &parsed.history {
        match entry.kind {
            HistoryKind::User => {
                flush_turn(blob_store, &mut turn_blob_ids, &mut current_turn);
                let user = UserMessage {
                    text: entry.text.clone(),
                    message_id: deterministic_uuid(&format!(
                        "u:{}:{}",
                        turn_blob_ids.len(),
                        entry.text
                    )),
                };
                current_turn = Some((blob_store.store(&user.encode_to_vec()), Vec::new()));
            }
            HistoryKind::Assistant | HistoryKind::Tool => {
                if current_turn.is_none() {
                    let user = UserMessage {
                        text: String::new(),
                        message_id: deterministic_uuid(&format!("u:{}:", turn_blob_ids.len())),
                    };
                    current_turn = Some((blob_store.store(&user.encode_to_vec()), Vec::new()));
                }
                if let Some((_, steps)) = current_turn.as_mut() {
                    let step = ConversationStep {
                        message: Some(conversation_step::Message::AssistantMessage(
                            AssistantMessage {
                                text: entry.text.clone(),
                            },
                        )),
                    };
                    steps.push(blob_store.store(&step.encode_to_vec()));
                }
            }
        }
    }
    flush_turn(blob_store, &mut turn_blob_ids, &mut current_turn);

    let cursor_model_id = wire_model_id(model, speed, effort);
    let display_name = if model == "auto" {
        "Auto".to_string()
    } else {
        catalog_model_id(model).to_string()
    };
    let action = ConversationAction {
        action: Some(conversation_action::Action::UserMessageAction(
            UserMessageAction {
                user_message: Some(UserMessage {
                    text: parsed.user_text.clone(),
                    message_id: random_uuid(),
                }),
            },
        )),
    };
    let run = AgentRunRequest {
        conversation_state: Some(ConversationStateStructure {
            root_prompt_messages_json,
            turns: turn_blob_ids,
            token_details: None,
        }),
        action: Some(action),
        model_details: Some(ModelDetails {
            model_id: cursor_model_id.clone(),
            display_model_id: cursor_model_id.clone(),
            display_name: display_name.clone(),
            display_name_short: display_name,
            thinking_details: None,
        }),
        conversation_id: Some(conversation_id.to_string()),
        requested_model: Some(RequestedModel {
            model_id: cursor_model_id,
        }),
    };
    Ok(encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::RunRequest(run)),
    }))
}

fn store_prompt_blob(blob_store: &mut BlobStore, entry: &HistoryEntry) -> Vec<u8> {
    let role = match entry.kind {
        HistoryKind::Assistant => "assistant",
        HistoryKind::User | HistoryKind::Tool => "user",
    };
    blob_store.store(
        json!({
            "role": role,
            "content": [{ "type": "text", "text": entry.text }],
        })
        .to_string()
        .as_bytes(),
    )
}

fn parse_messages(messages: &[ChatMessage]) -> ParsedMessages {
    let mut system_prompts = Vec::new();
    let mut history = Vec::new();
    for message in messages {
        match message {
            ChatMessage::System(text) => {
                let text = text.trim();
                if !text.is_empty() {
                    system_prompts.push(text.to_string());
                }
            }
            ChatMessage::User(blocks) => history.push(HistoryEntry {
                kind: HistoryKind::User,
                text: text_from_user_blocks(blocks),
            }),
            ChatMessage::Assistant(blocks) => push_assistant(&mut history, blocks),
            ChatMessage::EnrichedAssistant(message) => {
                push_assistant(&mut history, &message.content)
            }
            ChatMessage::AbortedAssistant(message) => {
                push_assistant(&mut history, &message.content)
            }
            ChatMessage::ToolResult(result) => history.push(HistoryEntry {
                kind: HistoryKind::Tool,
                text: format_tool_result(result),
            }),
        }
    }
    let user_text = take_action_text(&mut history);
    ParsedMessages {
        system_prompts,
        user_text,
        history,
    }
}

fn take_action_text(history: &mut Vec<HistoryEntry>) -> String {
    if matches!(history.last(), Some(entry) if entry.kind == HistoryKind::User) {
        return history.pop().expect("checked last is user").text;
    }
    let mut trailing = Vec::new();
    while matches!(history.last(), Some(entry) if entry.kind == HistoryKind::Tool) {
        trailing.push(history.pop().expect("checked last is tool").text);
    }
    trailing.reverse();
    if !trailing.is_empty() {
        return trailing.join("\n\n");
    }
    if history.is_empty() {
        String::new()
    } else {
        CONTINUE_ACTION.to_string()
    }
}

fn push_assistant(history: &mut Vec<HistoryEntry>, blocks: &[ContentBlock]) {
    let text = transcript_from_blocks(blocks);
    if !text.is_empty() {
        history.push(HistoryEntry {
            kind: HistoryKind::Assistant,
            text,
        });
    }
}

fn text_from_user_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn transcript_from_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) if !text.trim().is_empty() => Some(text.clone()),
            ContentBlock::ToolCall(call) => Some(format_tool_call(call)),
            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_tool_call(call: &ToolCall) -> String {
    let args = serde_json::to_string_pretty(&call.arguments)
        .unwrap_or_else(|_| call.arguments.to_string());
    format!("[Called {} id={}]\n{args}", call.name, call.id)
}

fn format_tool_result(result: &ToolResult) -> String {
    let status = if result.ok { "ok" } else { "error" };
    if result.content.is_empty() {
        format!("[Tool Result id={} {status}]", result.id)
    } else {
        format!(
            "[Tool Result id={} {status}]\n{}",
            result.id, result.content
        )
    }
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod tests;
