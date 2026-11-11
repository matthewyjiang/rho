//! Read the loaded session's MCP servers without locking its runtime: their
//! inventory, what they offer a composer, one resource, and the values one
//! suggests for a prompt argument.

use std::sync::PoisonError;

use rho_protocol::{
    ErrorCode, RpcError, SessionId, SessionMcpCatalog, SessionMcpCatalogParams,
    SessionMcpCompleteParams, SessionMcpCompleteResponse, SessionMcpInventory, SessionMcpParams,
    SessionMcpResourceContent, SessionMcpResourceParams, SessionMcpResourceResponse,
    MAX_LINE_BYTES,
};

use super::Sessions;
use crate::{
    app::session_mcp::SessionMcp,
    tools::mcp::{McpCatalogError, McpResourceContent},
};

/// Room left on a response's line for what wraps its result: the JSON-RPC
/// envelope, about 40 bytes, and the request id a client chose.
const RESPONSE_ENVELOPE_BYTES: usize = 1024;

impl Sessions {
    pub(in crate::app::daemon_host) async fn mcp(
        &self,
        params: SessionMcpParams,
    ) -> Result<SessionMcpInventory, RpcError> {
        Ok(self.loaded_mcp(&params.session).await?.snapshot())
    }

    pub(in crate::app::daemon_host) async fn mcp_catalog(
        &self,
        params: SessionMcpCatalogParams,
    ) -> Result<SessionMcpCatalog, RpcError> {
        Ok(self.loaded_mcp(&params.session).await?.offered())
    }

    pub(in crate::app::daemon_host) async fn mcp_resource(
        &self,
        params: SessionMcpResourceParams,
    ) -> Result<SessionMcpResourceResponse, RpcError> {
        let mcp = self.loaded_mcp(&params.session).await?;
        let contents = mcp
            .servers()
            .read_resource(&params.server, &params.uri)
            .await
            .map_err(|error| {
                let code = match error {
                    McpCatalogError::UnknownServer(_) => ErrorCode::InvalidParams,
                    McpCatalogError::Server(_) => ErrorCode::InternalError,
                };
                RpcError::new(code, error.to_string())
            })?;
        let response = SessionMcpResourceResponse {
            contents: contents.into_iter().map(content_to_wire).collect(),
        };
        // A longer line would end the connection, and the session's
        // attachment with it, so the read is refused instead.
        let limit = MAX_LINE_BYTES - RESPONSE_ENVELOPE_BYTES;
        let size = serde_json::to_vec(&response)
            .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))?
            .len();
        if size > limit {
            return Err(RpcError::new(
                ErrorCode::InternalError,
                format!(
                    "resource is {size} bytes as a response, over the {limit}-byte limit \
                     (MAX_LINE_BYTES less RESPONSE_ENVELOPE_BYTES)"
                ),
            ));
        }
        Ok(response)
    }

    pub(in crate::app::daemon_host) async fn mcp_complete(
        &self,
        params: SessionMcpCompleteParams,
    ) -> Result<SessionMcpCompleteResponse, RpcError> {
        let mcp = self.loaded_mcp(&params.session).await?;
        Ok(SessionMcpCompleteResponse {
            values: mcp
                .servers()
                .complete_prompt_argument(
                    &params.server,
                    &params.prompt,
                    &params.argument,
                    &params.value,
                )
                .await,
        })
    }

    /// The handles to a loaded session's MCP servers, which outlive the
    /// lookup: a server the session has let go of since then refuses.
    async fn loaded_mcp(&self, session: &SessionId) -> Result<SessionMcp, RpcError> {
        self.refuse_when_stopping()?;
        let not_loaded = || {
            RpcError::new(
                ErrorCode::SessionNotLoaded,
                "the daemon has not loaded the session; attach to it first",
            )
        };
        let session = self.loaded_session(session).await?.ok_or_else(not_loaded)?;
        // An idle unload may have taken it since the lookup. Reading needs
        // no residency hold, since teardown does not change the inventory.
        let turn = session.enter().ok_or_else(not_loaded)?;
        let mcp = session
            .mcp
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        drop(turn);
        Ok(mcp)
    }
}

fn content_to_wire(content: McpResourceContent) -> SessionMcpResourceContent {
    match content {
        McpResourceContent::Text {
            uri,
            mime_type,
            text,
        } => SessionMcpResourceContent::Text {
            uri,
            mime_type,
            text,
        },
        McpResourceContent::Blob {
            uri,
            mime_type,
            blob,
        } => SessionMcpResourceContent::Blob {
            uri,
            mime_type,
            blob,
        },
        McpResourceContent::Unsupported => SessionMcpResourceContent::Unknown,
    }
}
