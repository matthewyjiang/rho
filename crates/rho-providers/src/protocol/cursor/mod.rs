//! Cursor AgentService Connect/protobuf wire protocol.
//!
//! Translates Rho model requests into Cursor `AgentService/Run` messages and
//! parses the bidirectional Connect stream. Credentials, HTTP, and retries stay
//! in the Cursor provider runtime.

pub(crate) mod connect;
pub(crate) mod convert;
pub(crate) mod fast;
pub(crate) mod proto;
pub(crate) mod value;

pub(crate) use connect::{decode_connect_unary_body, ConnectFrameParser, CONNECT_END_STREAM_FLAG};
pub(crate) use convert::{
    build_cursor_turn, decode_mcp_args, fallback_models, heartbeat_frame, kv_get_blob_response,
    kv_set_blob_response, models_from_details, native_exec_reject, request_context_success,
    CursorModel, CursorTurn,
};
pub(crate) use fast::{catalog_model_id, supports_fast_mode, CursorSpeed};
pub(crate) use proto::{
    agent_server_message, exec_server_message, interaction_update, kv_server_message,
    AgentServerMessage, GetUsableModelsRequest, GetUsableModelsResponse,
};

#[cfg(test)]
#[path = "connect_tests.rs"]
mod connect_tests;

#[cfg(test)]
#[path = "convert_tests.rs"]
mod convert_tests;
