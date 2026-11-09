//! `rho mcp serve`: the session-control core (`app::session_control`) as an
//! MCP server, over stdio or streamable HTTP (`http_server.rs`).
//!
//! The server is a thin adapter: each MCP tool is one core action, its
//! input schema the action's params, its result the action's JSON. It adds
//! no policy. Over stdio the caller is whoever started the process, with a
//! [`Grant::local`]; over HTTP each request's bearer token names its grant
//! (`tokens.rs`), which the core enforces.
//!
//! No tool answers a session's approvals or questions: the core has no such
//! action, and an unknown tool name is refused before anything runs.

use std::sync::Arc;

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
        ToolAnnotations,
    },
    service::RequestContext,
    ErrorData, RoleServer, ServerHandler, ServiceExt,
};

use super::session_control::{Access, Action, Grant, SessionControl};

mod http_server;
pub(crate) mod token_store;
pub(crate) mod tokens;

pub(super) use http_server::{serve_http, HttpOptions};

/// What the server tells a host about itself.
const INSTRUCTIONS: &str = "Start, drive, and follow rho agent sessions through the rho daemon. \
    `new` starts a session in a workspace with a first prompt; `wait` follows its turn until it \
    ends, returning a cursor to call again while `done` is false; `read` pages through a \
    transcript. A turn that needs an approval or an answer waits for a person in rho: no tool \
    here can answer it.";

/// The MCP handler: every tool call goes to the core under the caller's
/// grant.
#[derive(Clone)]
pub(super) struct SessionTools {
    control: Arc<SessionControl>,
    grants: Grants,
}

/// Where a call's grant comes from.
#[derive(Clone)]
enum Grants {
    /// One grant for every call, as over stdio.
    Fixed(Grant),
    /// The grant the HTTP layer put on the request after it checked the
    /// bearer token. A request without one is refused.
    PerRequest,
}

impl SessionTools {
    fn grant(&self, context: &RequestContext<RoleServer>) -> Result<Grant, ErrorData> {
        match &self.grants {
            Grants::Fixed(grant) => Ok(grant.clone()),
            Grants::PerRequest => context
                .extensions
                .get::<::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<Grant>())
                .cloned()
                .ok_or_else(|| ErrorData::invalid_request("the request carries no token", None)),
        }
    }
}

impl ServerHandler for SessionTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rho", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            Action::ALL.into_iter().map(tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(action) = Action::from_name(&request.name) else {
            return Err(ErrorData::invalid_params(
                format!("no tool named {:?}", request.name),
                None,
            ));
        };
        let grant = self.grant(&context)?;
        let arguments = request.arguments.unwrap_or_default();
        let result = match self.control.call(&grant, action, arguments).await {
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value)
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            // Refusals and failures go to the model, which can act on them.
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
        };
        Ok(result.into())
    }
}

/// The MCP tool for `action`.
fn tool(action: Action) -> Tool {
    let annotations =
        ToolAnnotations::with_title(action.name()).read_only(action.needs() == Access::Read);
    Tool::new(action.name(), action.description(), action.input_schema())
        .with_annotations(annotations)
}

/// Serves the tools over stdio for the process's caller until the host
/// closes stdin.
pub(super) async fn serve_stdio(control: Arc<SessionControl>) -> anyhow::Result<()> {
    let tools = SessionTools {
        control,
        grants: Grants::Fixed(Grant::local()),
    };
    let running = tools.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
