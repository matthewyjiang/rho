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
        let max_text_bytes = params
            .max_text_bytes
            .map(|max| usize::try_from(max).unwrap_or(usize::MAX));
        let response = SessionMcpResourceResponse {
            contents: contents
                .into_iter()
                .map(|content| match (content, max_text_bytes) {
                    (
                        McpResourceContent::Text {
                            uri,
                            mime_type,
                            text,
                        },
                        Some(max),
                    ) => McpResourceContent::Text {
                        uri,
                        mime_type,
                        text: cut_text(text, max),
                    },
                    (content, _) => content,
                })
                .map(content_to_wire)
                .collect(),
        };
        // A longer line would end the connection, and the session's
        // attachment with it, so what still does not fit is refused.
        let limit = MAX_LINE_BYTES - RESPONSE_ENVELOPE_BYTES;
        let size = serde_json::to_vec(&response)
            .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))?
            .len();
        if size > limit {
            return Err(RpcError::new(
                ErrorCode::InternalError,
                format!(
                    "resource is {size} bytes as a response, over the daemon protocol's \
                     {limit}-byte response limit"
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

/// The longest a UTF-8 character encodes to.
const UTF8_CHAR_BYTES: usize = 4;

/// Cuts `text` for a client that uses at most `max` bytes of it. The cut
/// keeps those bytes and at least one more, so the client's own cut lands
/// where it would on the whole text and still finds there was more.
fn cut_text(mut text: String, max: usize) -> String {
    let keep = max.saturating_add(UTF8_CHAR_BYTES);
    if text.len() > keep {
        text.truncate(rho_sdk::floor_char_boundary(&text, keep));
    }
    text
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

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
