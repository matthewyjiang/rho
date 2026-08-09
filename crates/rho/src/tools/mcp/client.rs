//! Rho's MCP client handler: identity, declared capabilities, and the
//! server-initiated requests and notifications Rho answers.
//!
//! rmcp's default client is an inert [`ClientInfo`] that identifies itself with
//! rmcp's own build metadata and declares no capabilities. Servers use both to
//! decide what they may ask for, so Rho supplies its own name, version, and
//! capability set, and answers the server-to-client traffic that follows.

// `roots` and `logging` carry SEP-2577 deprecation markers in rmcp while every
// shipping server still uses them. Rho implements the current wire protocol.
#![expect(deprecated)]

use std::future::Future;

use rmcp::{
    model::{
        ClientCapabilities, ClientInfo, Implementation, ListRootsResult, LoggingLevel,
        LoggingMessageNotificationParam, ProgressNotificationParam, RootsCapabilities,
    },
    service::{NotificationContext, RequestContext, RoleClient},
    ClientHandler, ErrorData as McpError,
};

use super::{progress::McpProgressRouter, roots::McpRoots};

/// A server-initiated change that the owning session must act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpServerEvent {
    /// `notifications/tools/list_changed`: re-run discovery for this server.
    ToolsChanged,
}

pub(crate) type McpEventSender = tokio::sync::mpsc::UnboundedSender<McpServerEvent>;
pub(crate) type McpEventReceiver = tokio::sync::mpsc::UnboundedReceiver<McpServerEvent>;

/// Client handler for one MCP server session.
pub(crate) struct McpClientHandler {
    identity: String,
    info: ClientInfo,
    roots: McpRoots,
    progress: McpProgressRouter,
    events: McpEventSender,
}

impl McpClientHandler {
    pub(crate) fn new(
        identity: impl Into<String>,
        roots: McpRoots,
        progress: McpProgressRouter,
        events: McpEventSender,
    ) -> Self {
        let identity = identity.into();
        Self {
            info: client_info(&roots),
            identity,
            roots,
            progress,
            events,
        }
    }
}

/// Rho's `initialize` payload. The capability set is the contract servers read
/// before they send anything back, so it must list exactly what this handler
/// answers.
fn client_info(roots: &McpRoots) -> ClientInfo {
    let mut capabilities = ClientCapabilities::default();
    // Advertise roots only when there is a workspace to advertise. A server
    // that sees the capability may reasonably expect a non-empty list.
    if !roots.is_empty() {
        let mut declared = RootsCapabilities::default();
        // Rho's workspace is fixed for a session, so it never sends
        // `notifications/roots/list_changed`.
        declared.list_changed = Some(false);
        capabilities.roots = Some(declared);
    }
    ClientInfo::new(
        capabilities,
        Implementation::new("rho", env!("CARGO_PKG_VERSION"))
            .with_title("Rho")
            .with_description("Rho coding agent")
            .with_website_url(RHO_WEBSITE),
    )
}

const RHO_WEBSITE: &str = "https://github.com/matthewyjiang/rho";

impl ClientHandler for McpClientHandler {
    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }

    fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListRootsResult, McpError>> + Send + '_ {
        let roots = self.roots.to_protocol();
        std::future::ready(Ok(ListRootsResult::new(roots)))
    }

    fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.progress.dispatch(params)
    }

    fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        log_server_message(&self.identity, params);
        std::future::ready(())
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        // A closed receiver means the session is shutting down; the change has
        // no one left to apply it.
        let _ = self.events.send(McpServerEvent::ToolsChanged);
        std::future::ready(())
    }
}

/// Server logs enter Rho's own tracing output under a fixed target so they can
/// be filtered apart from Rho's logs. MCP severities above `error` have no
/// tracing equivalent and map onto `error`.
fn log_server_message(identity: &str, params: LoggingMessageNotificationParam) {
    let logger = params.logger.unwrap_or_else(|| "mcp".into());
    let data = match &params.data {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    match params.level {
        LoggingLevel::Debug => {
            tracing::debug!(target: "rho::mcp::server", server = %identity, logger = %logger, "{data}");
        }
        LoggingLevel::Info | LoggingLevel::Notice => {
            tracing::info!(target: "rho::mcp::server", server = %identity, logger = %logger, "{data}");
        }
        LoggingLevel::Warning => {
            tracing::warn!(target: "rho::mcp::server", server = %identity, logger = %logger, "{data}");
        }
        LoggingLevel::Error
        | LoggingLevel::Critical
        | LoggingLevel::Alert
        | LoggingLevel::Emergency => {
            tracing::error!(target: "rho::mcp::server", server = %identity, logger = %logger, "{data}");
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
