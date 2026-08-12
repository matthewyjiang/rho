use std::collections::BTreeMap;

use prost::Message;
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::model::{ModelError, ModelIdentity, ModelRequest, ToolCall, ToolSpec};
use crate::protocol::openai_chat::{to_openai_message_for_target, OpenAiMessage};

use super::connect::encode_connect_frame;
use super::proto::{
    agent_client_message, conversation_action, conversation_step, conversation_turn_structure,
    exec_client_message, exec_server_message, kv_client_message, AgentClientMessage,
    AgentConversationTurnStructure, AgentRunRequest, AssistantMessage, BackgroundShellSpawnResult,
    ClientHeartbeat, ConversationAction, ConversationStateStructure, ConversationStep,
    ConversationTurnStructure, DeleteResult, DiagnosticsResult, ExecClientMessage,
    ExecServerMessage, FetchError, FetchResult, GetBlobResult, GrepError, GrepResult,
    KvClientMessage, LsResult, McpArgs, McpToolDefinition, ModelDetails, PathRejected, ReadResult,
    RequestContext, RequestContextResult, RequestContextSuccess, RequestedModel, ResumeAction,
    SetBlobResult, ShellRejected, ShellResult, UserMessage, UserMessageAction, WriteResult,
    WriteShellStdinError, WriteShellStdinResult,
};
use super::value::{json_from_protobuf_value, protobuf_value_from_json};

pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 64_000;
const MCP_PROVIDER: &str = "rho";
const NATIVE_REJECT_REASON: &str =
    "Tool not available in this environment. Use the MCP tools provided instead.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorModel {
    pub id: String,
    pub name: String,
    pub reasoning: bool,
    pub context_window: u64,
    pub max_tokens: u64,
}

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

pub(crate) fn fallback_models() -> Vec<CursorModel> {
    ensure_auto_model(
        [
            (
                "composer-1",
                "Composer 1",
                true,
                DEFAULT_CONTEXT_WINDOW,
                DEFAULT_MAX_OUTPUT_TOKENS,
            ),
            (
                "composer-1.5",
                "Composer 1.5",
                true,
                DEFAULT_CONTEXT_WINDOW,
                DEFAULT_MAX_OUTPUT_TOKENS,
            ),
            (
                "claude-4.6-opus-high",
                "Claude 4.6 Opus",
                true,
                DEFAULT_CONTEXT_WINDOW,
                128_000,
            ),
            (
                "claude-4.6-sonnet-medium",
                "Claude 4.6 Sonnet",
                true,
                DEFAULT_CONTEXT_WINDOW,
                DEFAULT_MAX_OUTPUT_TOKENS,
            ),
            (
                "claude-4.5-sonnet",
                "Claude 4.5 Sonnet",
                true,
                DEFAULT_CONTEXT_WINDOW,
                DEFAULT_MAX_OUTPUT_TOKENS,
            ),
            ("gpt-5.4-medium", "GPT-5.4", true, 272_000, 128_000),
            ("gpt-5.2", "GPT-5.2", true, 400_000, 128_000),
            ("gpt-5.2-codex", "GPT-5.2 Codex", true, 400_000, 128_000),
            ("gpt-5.3-codex", "GPT-5.3 Codex", true, 400_000, 128_000),
            (
                "gpt-5.3-codex-spark-preview",
                "GPT-5.3 Codex Spark",
                true,
                128_000,
                128_000,
            ),
            (
                "gemini-3.1-pro",
                "Gemini 3.1 Pro",
                true,
                1_000_000,
                DEFAULT_MAX_OUTPUT_TOKENS,
            ),
            (
                "grok-code-fast-1",
                "Grok Code Fast 1",
                false,
                128_000,
                DEFAULT_MAX_OUTPUT_TOKENS,
            ),
        ]
        .into_iter()
        .map(
            |(id, name, reasoning, context_window, max_tokens)| CursorModel {
                id: id.into(),
                name: name.into(),
                reasoning,
                context_window,
                max_tokens,
            },
        )
        .collect(),
    )
}

pub(crate) fn models_from_details(details: &[ModelDetails]) -> Vec<CursorModel> {
    let mut models = details
        .iter()
        .filter_map(|details| {
            let id = details.model_id.trim();
            if id.is_empty() {
                return None;
            }
            let name = [
                &details.display_name,
                &details.display_name_short,
                &details.display_model_id,
            ]
            .into_iter()
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
            .unwrap_or(id);
            Some(CursorModel {
                id: id.to_string(),
                name: name.to_string(),
                reasoning: details.thinking_details.is_some(),
                context_window: DEFAULT_CONTEXT_WINDOW,
                max_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    ensure_auto_model(models)
}

fn ensure_auto_model(mut models: Vec<CursorModel>) -> Vec<CursorModel> {
    if !models.iter().any(|model| model.id == "auto") {
        models.insert(
            0,
            CursorModel {
                id: "auto".into(),
                name: "Auto".into(),
                reasoning: false,
                context_window: DEFAULT_CONTEXT_WINDOW,
                max_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            },
        );
    }
    models
}

pub(crate) fn build_cursor_turn(
    identity: &ModelIdentity,
    model: &str,
    request: ModelRequest<'_>,
) -> Result<CursorTurn, ModelError> {
    let messages = request
        .messages
        .iter()
        .cloned()
        .map(|message| to_openai_message_for_target(message, Some(identity)))
        .collect::<Result<Vec<_>, _>>()?;
    let parsed = parse_messages(&messages);
    if parsed.user_text.is_empty() && parsed.history.is_empty() {
        return Err(ModelError::InvalidResponse(
            "Cursor request has no user message".into(),
        ));
    }
    let first_user = parsed
        .history
        .iter()
        .find(|entry| entry.kind == HistoryKind::User)
        .map(|entry| entry.text.as_str())
        .unwrap_or(parsed.user_text.as_str());
    let conversation_id = deterministic_uuid(&format!("cursor-conv-id:{first_user}"));
    let mut blob_store = BlobStore::default();
    let request_bytes = encode_run_request(model, &parsed, &conversation_id, &mut blob_store)?;
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

pub(crate) fn encode_client_message(message: &AgentClientMessage) -> Vec<u8> {
    encode_connect_frame(&message.encode_to_vec(), 0)
}

pub(crate) fn heartbeat_frame() -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::ClientHeartbeat(
            ClientHeartbeat {},
        )),
    })
}

pub(crate) fn kv_get_blob_response(id: u32, blob_data: Option<Vec<u8>>) -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::KvClientMessage(
            KvClientMessage {
                id,
                message: Some(kv_client_message::Message::GetBlobResult(GetBlobResult {
                    blob_data,
                })),
            },
        )),
    })
}

pub(crate) fn kv_set_blob_response(id: u32) -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::KvClientMessage(
            KvClientMessage {
                id,
                message: Some(kv_client_message::Message::SetBlobResult(SetBlobResult {})),
            },
        )),
    })
}

pub(crate) fn request_context_success(
    exec: &ExecServerMessage,
    tools: Vec<McpToolDefinition>,
    cloud_rule: Option<String>,
) -> Vec<u8> {
    encode_exec_result(
        exec,
        exec_client_message::Message::RequestContextResult(RequestContextResult {
            result: Some(super::proto::request_context_result::Result::Success(
                RequestContextSuccess {
                    request_context: Some(RequestContext { tools, cloud_rule }),
                },
            )),
        }),
    )
}

pub(crate) fn native_exec_reject(exec: &ExecServerMessage) -> Option<Vec<u8>> {
    let result = match exec.message.as_ref()? {
        exec_server_message::Message::ReadArgs(args) => {
            exec_client_message::Message::ReadResult(ReadResult {
                result: Some(super::proto::read_result::Result::Rejected(path_rejected(
                    &args.path,
                ))),
            })
        }
        exec_server_message::Message::LsArgs(args) => {
            exec_client_message::Message::LsResult(LsResult {
                result: Some(super::proto::ls_result::Result::Rejected(path_rejected(
                    &args.path,
                ))),
            })
        }
        exec_server_message::Message::GrepArgs(_) => {
            exec_client_message::Message::GrepResult(GrepResult {
                result: Some(super::proto::grep_result::Result::Error(GrepError {
                    error: NATIVE_REJECT_REASON.into(),
                })),
            })
        }
        exec_server_message::Message::WriteArgs(args) => {
            exec_client_message::Message::WriteResult(WriteResult {
                result: Some(super::proto::write_result::Result::Rejected(path_rejected(
                    &args.path,
                ))),
            })
        }
        exec_server_message::Message::DeleteArgs(args) => {
            exec_client_message::Message::DeleteResult(DeleteResult {
                result: Some(super::proto::delete_result::Result::Rejected(
                    path_rejected(&args.path),
                )),
            })
        }
        exec_server_message::Message::ShellArgs(args)
        | exec_server_message::Message::ShellStreamArgs(args) => {
            exec_client_message::Message::ShellResult(shell_rejected(
                args.command.clone(),
                args.working_directory.clone(),
            ))
        }
        exec_server_message::Message::BackgroundShellSpawnArgs(args) => {
            exec_client_message::Message::BackgroundShellSpawnResult(BackgroundShellSpawnResult {
                result: Some(
                    super::proto::background_shell_spawn_result::Result::Rejected(ShellRejected {
                        command: args.command.clone(),
                        working_directory: args.working_directory.clone(),
                        reason: NATIVE_REJECT_REASON.into(),
                        is_readonly: false,
                    }),
                ),
            })
        }
        exec_server_message::Message::FetchArgs(args) => {
            exec_client_message::Message::FetchResult(FetchResult {
                result: Some(super::proto::fetch_result::Result::Error(FetchError {
                    url: args.url.clone(),
                    error: NATIVE_REJECT_REASON.into(),
                })),
            })
        }
        exec_server_message::Message::WriteShellStdinArgs(_) => {
            exec_client_message::Message::WriteShellStdinResult(WriteShellStdinResult {
                result: Some(super::proto::write_shell_stdin_result::Result::Error(
                    WriteShellStdinError {
                        error: NATIVE_REJECT_REASON.into(),
                    },
                )),
            })
        }
        exec_server_message::Message::DiagnosticsArgs(_) => {
            exec_client_message::Message::DiagnosticsResult(DiagnosticsResult {})
        }
        exec_server_message::Message::RequestContextArgs(_)
        | exec_server_message::Message::McpArgs(_) => return None,
    };
    Some(encode_exec_result(exec, result))
}

pub(crate) fn decode_mcp_args(args: &McpArgs) -> Result<ToolCall, ModelError> {
    let mut object = serde_json::Map::new();
    for (key, value) in &args.args {
        object.insert(key.clone(), decode_mcp_arg_value(value));
    }
    let name = if args.tool_name.is_empty() {
        args.name.clone()
    } else {
        args.tool_name.clone()
    };
    let id = if args.tool_call_id.is_empty() {
        deterministic_uuid(&format!("mcp:{name}"))
    } else {
        args.tool_call_id.clone()
    };
    Ok(ToolCall {
        id,
        name,
        arguments: Value::Object(object),
    })
}

fn decode_mcp_arg_value(bytes: &[u8]) -> Value {
    if let Ok(parsed) = prost_types::Value::decode(bytes) {
        return json_from_protobuf_value(&parsed);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(json) = serde_json::from_str(text) {
            return json;
        }
        return Value::String(text.to_string());
    }
    Value::Null
}

fn encode_run_request(
    model: &str,
    parsed: &ParsedMessages,
    conversation_id: &str,
    blob_store: &mut BlobStore,
) -> Result<Vec<u8>, ModelError> {
    let prompts = if parsed.system_prompts.is_empty() {
        vec!["You are a helpful assistant.".to_string()]
    } else {
        parsed.system_prompts.clone()
    };
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
        let blob = match entry.kind {
            HistoryKind::Assistant => json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": entry.text }],
            }),
            HistoryKind::User | HistoryKind::Tool => {
                let text = if entry.kind == HistoryKind::Tool {
                    format!("[Tool Result]\n{}", entry.text)
                } else {
                    entry.text.clone()
                };
                json!({
                    "role": "user",
                    "content": [{ "type": "text", "text": text }],
                })
            }
        };
        root_prompt_messages_json.push(blob_store.store(blob.to_string().as_bytes()));
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
                if let Some((_, steps)) = current_turn.as_mut() {
                    let text = if entry.kind == HistoryKind::Tool {
                        format!("[Tool Result]\n{}", entry.text)
                    } else {
                        entry.text.clone()
                    };
                    let step = ConversationStep {
                        message: Some(conversation_step::Message::AssistantMessage(
                            AssistantMessage { text },
                        )),
                    };
                    steps.push(blob_store.store(&step.encode_to_vec()));
                }
            }
        }
    }
    flush_turn(blob_store, &mut turn_blob_ids, &mut current_turn);

    let cursor_model_id = if model == "auto" { "default" } else { model };
    let display_name = if model == "auto" { "Auto" } else { model };
    let action = if parsed.user_text.is_empty() {
        ConversationAction {
            action: Some(conversation_action::Action::ResumeAction(ResumeAction {})),
        }
    } else {
        ConversationAction {
            action: Some(conversation_action::Action::UserMessageAction(
                UserMessageAction {
                    user_message: Some(UserMessage {
                        text: parsed.user_text.clone(),
                        message_id: random_message_id(),
                    }),
                },
            )),
        }
    };
    let run = AgentRunRequest {
        conversation_state: Some(ConversationStateStructure {
            root_prompt_messages_json,
            turns: turn_blob_ids,
            token_details: None,
        }),
        action: Some(action),
        model_details: Some(ModelDetails {
            model_id: cursor_model_id.into(),
            display_model_id: cursor_model_id.into(),
            display_name: display_name.into(),
            display_name_short: display_name.into(),
            thinking_details: None,
        }),
        conversation_id: Some(conversation_id.to_string()),
        requested_model: Some(RequestedModel {
            model_id: cursor_model_id.into(),
        }),
    };
    Ok(encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::RunRequest(run)),
    }))
}

fn parse_messages(messages: &[OpenAiMessage]) -> ParsedMessages {
    let system_prompts = messages
        .iter()
        .filter(|message| message.role == "system")
        .map(openai_text)
        .filter(|text| !text.is_empty())
        .collect();
    let mut history = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "tool" => {
                let text = openai_text(message);
                if !text.is_empty() {
                    history.push(HistoryEntry {
                        kind: HistoryKind::Tool,
                        text,
                    });
                }
            }
            "user" => history.push(HistoryEntry {
                kind: HistoryKind::User,
                text: openai_text(message),
            }),
            "assistant" => {
                let text = openai_text(message);
                if !text.is_empty() {
                    history.push(HistoryEntry {
                        kind: HistoryKind::Assistant,
                        text,
                    });
                }
            }
            _ => {}
        }
    }
    let mut user_text = String::new();
    if matches!(history.last(), Some(entry) if entry.kind == HistoryKind::User) {
        user_text = history.pop().expect("checked last is user").text;
    }
    ParsedMessages {
        system_prompts,
        user_text,
        history,
    }
}

fn openai_text(message: &OpenAiMessage) -> String {
    match &message.content {
        None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.as_str().unwrap_or("").to_string(),
    }
}

fn mcp_tool_definitions(tools: &[ToolSpec]) -> Vec<McpToolDefinition> {
    tools
        .iter()
        .map(|tool| McpToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: protobuf_value_from_json(&tool.input_schema).encode_to_vec(),
            provider_identifier: MCP_PROVIDER.into(),
            tool_name: tool.name.clone(),
        })
        .collect()
}

fn encode_exec_result(exec: &ExecServerMessage, message: exec_client_message::Message) -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::ExecClientMessage(
            ExecClientMessage {
                id: exec.id,
                exec_id: exec.exec_id.clone(),
                message: Some(message),
            },
        )),
    })
}

fn path_rejected(path: &str) -> PathRejected {
    PathRejected {
        path: path.to_string(),
        reason: NATIVE_REJECT_REASON.into(),
    }
}

fn shell_rejected(command: String, working_directory: String) -> ShellResult {
    ShellResult {
        result: Some(super::proto::shell_result::Result::Rejected(
            ShellRejected {
                command,
                working_directory,
                reason: NATIVE_REJECT_REASON.into(),
                is_readonly: false,
            },
        )),
    }
}

fn deterministic_uuid(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let hex = to_hex(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn random_message_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    deterministic_uuid(&to_hex(&bytes))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
