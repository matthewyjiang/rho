//! Minimal Cursor `agent.v1` messages used by Rho's AgentService client.
//!
//! Field numbers match Cursor's published protobuf so unknown server fields can
//! be skipped. Only the shapes Rho sends or matches are modeled.

#![allow(clippy::enum_variant_names)]

use std::collections::BTreeMap;

use prost::Message;

#[derive(Clone, PartialEq, Message)]
pub struct AgentClientMessage {
    #[prost(oneof = "agent_client_message::Message", tags = "1, 2, 3, 7")]
    pub message: Option<agent_client_message::Message>,
}

pub mod agent_client_message {
    use super::{AgentRunRequest, ClientHeartbeat, ExecClientMessage, KvClientMessage};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "1")]
        RunRequest(AgentRunRequest),
        #[prost(message, tag = "2")]
        ExecClientMessage(ExecClientMessage),
        #[prost(message, tag = "3")]
        KvClientMessage(KvClientMessage),
        #[prost(message, tag = "7")]
        ClientHeartbeat(ClientHeartbeat),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct AgentServerMessage {
    #[prost(oneof = "agent_server_message::Message", tags = "1, 2, 3, 4")]
    pub message: Option<agent_server_message::Message>,
}

pub mod agent_server_message {
    use super::{
        ConversationStateStructure, ExecServerMessage, InteractionUpdate, KvServerMessage,
    };
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "1")]
        InteractionUpdate(InteractionUpdate),
        #[prost(message, tag = "2")]
        ExecServerMessage(ExecServerMessage),
        #[prost(message, tag = "3")]
        ConversationCheckpointUpdate(ConversationStateStructure),
        #[prost(message, tag = "4")]
        KvServerMessage(KvServerMessage),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct AgentRunRequest {
    #[prost(message, optional, tag = "1")]
    pub conversation_state: Option<ConversationStateStructure>,
    #[prost(message, optional, tag = "2")]
    pub action: Option<ConversationAction>,
    #[prost(message, optional, tag = "3")]
    pub model_details: Option<ModelDetails>,
    #[prost(string, optional, tag = "5")]
    pub conversation_id: Option<String>,
    #[prost(message, optional, tag = "9")]
    pub requested_model: Option<RequestedModel>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ConversationAction {
    #[prost(oneof = "conversation_action::Action", tags = "1, 2")]
    pub action: Option<conversation_action::Action>,
}

pub mod conversation_action {
    use super::{ResumeAction, UserMessageAction};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Action {
        #[prost(message, tag = "1")]
        UserMessageAction(UserMessageAction),
        #[prost(message, tag = "2")]
        ResumeAction(ResumeAction),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct UserMessageAction {
    #[prost(message, optional, tag = "1")]
    pub user_message: Option<UserMessage>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ResumeAction {}

#[derive(Clone, PartialEq, Message)]
pub struct UserMessage {
    #[prost(string, tag = "1")]
    pub text: String,
    #[prost(string, tag = "2")]
    pub message_id: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ConversationStateStructure {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub root_prompt_messages_json: Vec<Vec<u8>>,
    #[prost(bytes = "vec", repeated, tag = "8")]
    pub turns: Vec<Vec<u8>>,
    #[prost(message, optional, tag = "5")]
    pub token_details: Option<ConversationTokenDetails>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ConversationTokenDetails {
    #[prost(uint32, tag = "1")]
    pub used_tokens: u32,
    #[prost(uint32, tag = "2")]
    pub max_tokens: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ConversationTurnStructure {
    #[prost(oneof = "conversation_turn_structure::Turn", tags = "1")]
    pub turn: Option<conversation_turn_structure::Turn>,
}

pub mod conversation_turn_structure {
    use super::AgentConversationTurnStructure;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Turn {
        #[prost(message, tag = "1")]
        AgentConversationTurn(AgentConversationTurnStructure),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct AgentConversationTurnStructure {
    #[prost(bytes = "vec", tag = "1")]
    pub user_message: Vec<u8>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub steps: Vec<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ConversationStep {
    #[prost(oneof = "conversation_step::Message", tags = "1")]
    pub message: Option<conversation_step::Message>,
}

pub mod conversation_step {
    use super::AssistantMessage;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "1")]
        AssistantMessage(AssistantMessage),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct AssistantMessage {
    #[prost(string, tag = "1")]
    pub text: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestedModel {
    #[prost(string, tag = "1")]
    pub model_id: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ModelDetails {
    #[prost(string, tag = "1")]
    pub model_id: String,
    #[prost(string, tag = "3")]
    pub display_model_id: String,
    #[prost(string, tag = "4")]
    pub display_name: String,
    #[prost(string, tag = "5")]
    pub display_name_short: String,
    #[prost(message, optional, tag = "2")]
    pub thinking_details: Option<ThinkingDetails>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ThinkingDetails {}

#[derive(Clone, PartialEq, Message)]
pub struct ClientHeartbeat {}

#[derive(Clone, PartialEq, Message)]
pub struct InteractionUpdate {
    #[prost(oneof = "interaction_update::Message", tags = "1, 4, 8, 14")]
    pub message: Option<interaction_update::Message>,
}

pub mod interaction_update {
    use super::{TextDeltaUpdate, ThinkingDeltaUpdate, TokenDeltaUpdate, TurnEndedUpdate};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "1")]
        TextDelta(TextDeltaUpdate),
        #[prost(message, tag = "4")]
        ThinkingDelta(ThinkingDeltaUpdate),
        #[prost(message, tag = "8")]
        TokenDelta(TokenDeltaUpdate),
        #[prost(message, tag = "14")]
        TurnEnded(TurnEndedUpdate),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct TextDeltaUpdate {
    #[prost(string, tag = "1")]
    pub text: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ThinkingDeltaUpdate {
    #[prost(string, tag = "1")]
    pub text: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct TokenDeltaUpdate {
    #[prost(int32, tag = "1")]
    pub tokens: i32,
}

#[derive(Clone, PartialEq, Message)]
pub struct TurnEndedUpdate {}

#[derive(Clone, PartialEq, Message)]
pub struct KvServerMessage {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(oneof = "kv_server_message::Message", tags = "2, 3")]
    pub message: Option<kv_server_message::Message>,
}

pub mod kv_server_message {
    use super::{GetBlobArgs, SetBlobArgs};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "2")]
        GetBlobArgs(GetBlobArgs),
        #[prost(message, tag = "3")]
        SetBlobArgs(SetBlobArgs),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct GetBlobArgs {
    #[prost(bytes = "vec", tag = "1")]
    pub blob_id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SetBlobArgs {
    #[prost(bytes = "vec", tag = "1")]
    pub blob_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub blob_data: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct KvClientMessage {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(oneof = "kv_client_message::Message", tags = "2, 3")]
    pub message: Option<kv_client_message::Message>,
}

pub mod kv_client_message {
    use super::{GetBlobResult, SetBlobResult};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "2")]
        GetBlobResult(GetBlobResult),
        #[prost(message, tag = "3")]
        SetBlobResult(SetBlobResult),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct GetBlobResult {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub blob_data: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SetBlobResult {}

#[derive(Clone, PartialEq, Message)]
pub struct ExecServerMessage {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(string, tag = "15")]
    pub exec_id: String,
    #[prost(
        oneof = "exec_server_message::Message",
        tags = "2, 3, 4, 5, 7, 8, 9, 10, 11, 14, 16, 20, 23"
    )]
    pub message: Option<exec_server_message::Message>,
}

pub mod exec_server_message {
    use super::{
        BackgroundShellSpawnArgs, DeleteArgs, DiagnosticsArgs, FetchArgs, GrepArgs, LsArgs,
        McpArgs, ReadArgs, RequestContextArgs, ShellArgs, WriteArgs, WriteShellStdinArgs,
    };
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "2")]
        ShellArgs(ShellArgs),
        #[prost(message, tag = "3")]
        WriteArgs(WriteArgs),
        #[prost(message, tag = "4")]
        DeleteArgs(DeleteArgs),
        #[prost(message, tag = "5")]
        GrepArgs(GrepArgs),
        #[prost(message, tag = "7")]
        ReadArgs(ReadArgs),
        #[prost(message, tag = "8")]
        LsArgs(LsArgs),
        #[prost(message, tag = "9")]
        DiagnosticsArgs(DiagnosticsArgs),
        #[prost(message, tag = "10")]
        RequestContextArgs(RequestContextArgs),
        #[prost(message, tag = "11")]
        McpArgs(McpArgs),
        #[prost(message, tag = "14")]
        ShellStreamArgs(ShellArgs),
        #[prost(message, tag = "16")]
        BackgroundShellSpawnArgs(BackgroundShellSpawnArgs),
        #[prost(message, tag = "20")]
        FetchArgs(FetchArgs),
        #[prost(message, tag = "23")]
        WriteShellStdinArgs(WriteShellStdinArgs),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestContextArgs {}

#[derive(Clone, PartialEq, Message)]
pub struct McpArgs {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(btree_map = "string, bytes", tag = "2")]
    pub args: BTreeMap<String, Vec<u8>>,
    #[prost(string, tag = "3")]
    pub tool_call_id: String,
    #[prost(string, tag = "4")]
    pub provider_identifier: String,
    #[prost(string, tag = "5")]
    pub tool_name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ReadArgs {
    #[prost(string, tag = "1")]
    pub path: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct LsArgs {
    #[prost(string, tag = "1")]
    pub path: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct GrepArgs {}

#[derive(Clone, PartialEq, Message)]
pub struct WriteArgs {
    #[prost(string, tag = "1")]
    pub path: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct DeleteArgs {
    #[prost(string, tag = "1")]
    pub path: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ShellArgs {
    #[prost(string, tag = "1")]
    pub command: String,
    #[prost(string, tag = "2")]
    pub working_directory: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct BackgroundShellSpawnArgs {
    #[prost(string, tag = "1")]
    pub command: String,
    #[prost(string, tag = "2")]
    pub working_directory: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct FetchArgs {
    #[prost(string, tag = "1")]
    pub url: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WriteShellStdinArgs {}

#[derive(Clone, PartialEq, Message)]
pub struct DiagnosticsArgs {}

#[derive(Clone, PartialEq, Message)]
pub struct ExecClientMessage {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    #[prost(string, tag = "15")]
    pub exec_id: String,
    #[prost(
        oneof = "exec_client_message::Message",
        tags = "2, 3, 4, 5, 7, 8, 9, 10, 11, 16, 20, 23"
    )]
    pub message: Option<exec_client_message::Message>,
}

pub mod exec_client_message {
    use super::{
        BackgroundShellSpawnResult, DeleteResult, DiagnosticsResult, FetchResult, GrepResult,
        LsResult, McpResult, ReadResult, RequestContextResult, ShellResult, WriteResult,
        WriteShellStdinResult,
    };
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Message {
        #[prost(message, tag = "2")]
        ShellResult(ShellResult),
        #[prost(message, tag = "3")]
        WriteResult(WriteResult),
        #[prost(message, tag = "4")]
        DeleteResult(DeleteResult),
        #[prost(message, tag = "5")]
        GrepResult(GrepResult),
        #[prost(message, tag = "7")]
        ReadResult(ReadResult),
        #[prost(message, tag = "8")]
        LsResult(LsResult),
        #[prost(message, tag = "9")]
        DiagnosticsResult(DiagnosticsResult),
        #[prost(message, tag = "10")]
        RequestContextResult(RequestContextResult),
        #[prost(message, tag = "11")]
        McpResult(McpResult),
        #[prost(message, tag = "16")]
        BackgroundShellSpawnResult(BackgroundShellSpawnResult),
        #[prost(message, tag = "20")]
        FetchResult(FetchResult),
        #[prost(message, tag = "23")]
        WriteShellStdinResult(WriteShellStdinResult),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestContextResult {
    #[prost(oneof = "request_context_result::Result", tags = "1")]
    pub result: Option<request_context_result::Result>,
}

pub mod request_context_result {
    use super::RequestContextSuccess;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(RequestContextSuccess),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestContextSuccess {
    #[prost(message, optional, tag = "1")]
    pub request_context: Option<RequestContext>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestContext {
    #[prost(message, repeated, tag = "7")]
    pub tools: Vec<McpToolDefinition>,
    #[prost(string, optional, tag = "16")]
    pub cloud_rule: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct McpToolDefinition {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub description: String,
    #[prost(bytes = "vec", tag = "3")]
    pub input_schema: Vec<u8>,
    #[prost(string, tag = "4")]
    pub provider_identifier: String,
    #[prost(string, tag = "5")]
    pub tool_name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct McpResult {}

#[derive(Clone, PartialEq, Message)]
pub struct ReadResult {
    #[prost(oneof = "read_result::Result", tags = "3")]
    pub result: Option<read_result::Result>,
}

pub mod read_result {
    use super::PathRejected;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "3")]
        Rejected(PathRejected),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct LsResult {
    #[prost(oneof = "ls_result::Result", tags = "3")]
    pub result: Option<ls_result::Result>,
}

pub mod ls_result {
    use super::PathRejected;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "3")]
        Rejected(PathRejected),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct PathRejected {
    #[prost(string, tag = "1")]
    pub path: String,
    #[prost(string, tag = "2")]
    pub reason: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct GrepResult {
    #[prost(oneof = "grep_result::Result", tags = "2")]
    pub result: Option<grep_result::Result>,
}

pub mod grep_result {
    use super::GrepError;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "2")]
        Error(GrepError),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct GrepError {
    #[prost(string, tag = "1")]
    pub error: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WriteResult {
    #[prost(oneof = "write_result::Result", tags = "6")]
    pub result: Option<write_result::Result>,
}

pub mod write_result {
    use super::PathRejected;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "6")]
        Rejected(PathRejected),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct DeleteResult {
    #[prost(oneof = "delete_result::Result", tags = "6")]
    pub result: Option<delete_result::Result>,
}

pub mod delete_result {
    use super::PathRejected;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "6")]
        Rejected(PathRejected),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct ShellResult {
    #[prost(oneof = "shell_result::Result", tags = "4")]
    pub result: Option<shell_result::Result>,
}

pub mod shell_result {
    use super::ShellRejected;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "4")]
        Rejected(ShellRejected),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct ShellRejected {
    #[prost(string, tag = "1")]
    pub command: String,
    #[prost(string, tag = "2")]
    pub working_directory: String,
    #[prost(string, tag = "3")]
    pub reason: String,
    #[prost(bool, tag = "4")]
    pub is_readonly: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct BackgroundShellSpawnResult {
    #[prost(oneof = "background_shell_spawn_result::Result", tags = "3")]
    pub result: Option<background_shell_spawn_result::Result>,
}

pub mod background_shell_spawn_result {
    use super::ShellRejected;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "3")]
        Rejected(ShellRejected),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct FetchResult {
    #[prost(oneof = "fetch_result::Result", tags = "2")]
    pub result: Option<fetch_result::Result>,
}

pub mod fetch_result {
    use super::FetchError;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "2")]
        Error(FetchError),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct FetchError {
    #[prost(string, tag = "1")]
    pub url: String,
    #[prost(string, tag = "2")]
    pub error: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct WriteShellStdinResult {
    #[prost(oneof = "write_shell_stdin_result::Result", tags = "2")]
    pub result: Option<write_shell_stdin_result::Result>,
}

pub mod write_shell_stdin_result {
    use super::WriteShellStdinError;
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "2")]
        Error(WriteShellStdinError),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WriteShellStdinError {
    #[prost(string, tag = "1")]
    pub error: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct DiagnosticsResult {}

#[derive(Clone, PartialEq, Message)]
pub struct GetUsableModelsRequest {}

#[derive(Clone, PartialEq, Message)]
pub struct GetUsableModelsResponse {
    #[prost(message, repeated, tag = "1")]
    pub models: Vec<ModelDetails>,
}
