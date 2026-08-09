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
        ClientCapabilities, ClientInfo, CreateMessageRequestParams, CreateMessageResult,
        ElicitRequestParams, ElicitResult, ElicitationCapability, FormElicitationCapability,
        Implementation, ListRootsResult, LoggingLevel, LoggingMessageNotificationParam,
        ProgressNotificationParam, RootsCapabilities, SamplingCapability,
    },
    service::{NotificationContext, RequestContext, RoleClient},
    ClientHandler, ErrorData as McpError,
};

use super::{
    elicitation::McpElicitationService, progress::McpProgressRouter, roots::McpRoots,
    sampling::McpSamplingService,
};

/// A server-initiated change that the owning session must act on.
///
/// The shared `Changed` suffix is deliberate: each variant is one
/// `notifications/<primitive>/list_changed`, and matching the protocol's own
/// naming is what makes the wire message and the variant obviously the same
/// thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum McpServerEvent {
    /// `notifications/tools/list_changed`: re-run discovery for this server.
    ToolsChanged,
    /// `notifications/prompts/list_changed`: re-list this server's prompts.
    PromptsChanged,
    /// `notifications/resources/list_changed`: re-list this server's resources.
    ResourcesChanged,
}

pub(crate) type McpEventSender = tokio::sync::mpsc::UnboundedSender<McpServerEvent>;
pub(crate) type McpEventReceiver = tokio::sync::mpsc::UnboundedReceiver<McpServerEvent>;

/// What one session may ask Rho to do beyond answering `roots/list`.
///
/// Bundled so the capability declaration and the handlers that honor it are
/// built from one value: a capability Rho declares but cannot serve is worse
/// than one it never offered.
pub(crate) struct McpClientServices {
    pub(crate) elicit: McpElicitationService,
    /// `Some` only when this server opted into sampling and this run can serve
    /// it.
    pub(crate) sample: Option<McpSamplingService>,
}

/// Client handler for one MCP server session.
pub(crate) struct McpClientHandler {
    identity: String,
    info: ClientInfo,
    roots: McpRoots,
    progress: McpProgressRouter,
    events: McpEventSender,
    services: McpClientServices,
}

impl McpClientHandler {
    pub(crate) fn new(
        identity: impl Into<String>,
        roots: McpRoots,
        progress: McpProgressRouter,
        events: McpEventSender,
        services: McpClientServices,
    ) -> Self {
        let identity = identity.into();
        Self {
            info: client_info(&roots, &services),
            identity,
            roots,
            progress,
            events,
            services,
        }
    }
}

/// Rho's `initialize` payload. The capability set is the contract servers read
/// before they send anything back, so it must list exactly what this handler
/// answers.
fn client_info(roots: &McpRoots, services: &McpClientServices) -> ClientInfo {
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
    if services.elicit.is_available() {
        // Form mode only: Rho has no way to send a person to a URL from a
        // background session. Schema validation is declared off because Rho
        // types answers to the schema but does not enforce its constraints.
        capabilities.elicitation = Some(
            ElicitationCapability::new()
                .with_form(FormElicitationCapability::new().with_schema_validation(false)),
        );
    }
    if services.sample.is_some() {
        // No sub-capabilities: Rho forwards neither tools nor server context
        // into a sampling request.
        capabilities.sampling = Some(SamplingCapability::default());
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

    fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ElicitResult, McpError>> + Send + '_ {
        self.services.elicit.elicit(request)
    }

    fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, McpError>> + Send + '_ {
        self.sample(params)
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
        self.announce(McpServerEvent::ToolsChanged)
    }

    fn on_prompt_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.announce(McpServerEvent::PromptsChanged)
    }

    fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.announce(McpServerEvent::ResourcesChanged)
    }
}

impl McpClientHandler {
    /// A server that did not opt into sampling gets the same answer as one
    /// talking to a client that never declared the capability, because as far
    /// as that server is concerned Rho does not implement the method.
    async fn sample(
        &self,
        params: CreateMessageRequestParams,
    ) -> Result<CreateMessageResult, McpError> {
        let Some(sampling) = self.services.sample.as_ref() else {
            return Err(McpError::method_not_found::<
                rmcp::model::CreateMessageRequestMethod,
            >());
        };
        sampling.create_message(params).await
    }

    /// Hand a server-announced change to the session's maintenance task. A
    /// closed receiver means the session is shutting down, so the change has no
    /// one left to apply it.
    fn announce(&self, event: McpServerEvent) -> std::future::Ready<()> {
        let _ = self.events.send(event);
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
