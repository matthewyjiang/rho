//! Cursor AgentService Connect/protobuf wire protocol.
//!
//! Translates Rho model requests into Cursor `AgentService/Run` messages and
//! parses the bidirectional Connect stream. Credentials, HTTP, and retries stay
//! in the Cursor provider runtime.

pub(crate) mod catalog;
pub(crate) mod connect;
pub(crate) mod effort;
pub(crate) mod exec;
pub(crate) mod fast;
pub(crate) mod ids;
pub(crate) mod mcp;
pub(crate) mod proto;
pub(crate) mod turn;
pub(crate) mod value;

pub(crate) use catalog::{fallback_models, models_from_details, CursorModel};
pub(crate) use connect::{decode_connect_unary_body, ConnectFrameParser, CONNECT_END_STREAM_FLAG};
pub(crate) use effort::CursorEffort;
pub(crate) use exec::{
    heartbeat_frame, kv_get_blob_response, kv_set_blob_response, native_exec_reject,
    request_context_success,
};
pub(crate) use fast::{catalog_model_id, supports_fast_mode, CursorSpeed};
pub(crate) use mcp::decode_mcp_args;
pub(crate) use proto::{
    agent_server_message, exec_server_message, interaction_update, kv_server_message,
    AgentServerMessage, GetUsableModelsRequest, GetUsableModelsResponse,
};
pub(crate) use turn::{build_cursor_turn, CursorTurn};

#[cfg(test)]
#[path = "connect_tests.rs"]
mod connect_tests;
