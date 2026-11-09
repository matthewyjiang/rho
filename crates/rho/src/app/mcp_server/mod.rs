//! `rho mcp serve`: the session-control core (`app::session_control`) as an
//! MCP server, over stdio or streamable HTTP (`http_server.rs`).
//!
//! The server is a thin adapter: each MCP tool is one core action, its
//! input schema the action's params, its result the action's JSON. It adds
//! no policy. Over stdio the caller is whoever started the process, with a
//! [`Grant::local`]; over HTTP each request's bearer token names its grant
//! (`tokens.rs`), which the core enforces.
//!
//! Each call's caller context (`crate::caller_context`) is the one a rho
//! session's MCP client sends in the request's `_meta`, which the server
//! declares it reads. Without one, a stdio call is from the context the
//! process started in, and an HTTP call from an outside agent's: depth 0,
//! whatever the listener's own. The context's scope narrows the grant; over
//! HTTP it can never widen a token's.
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

use super::session_control::{Action, Caller, Grant, SessionControl};
use crate::caller_context::{Access, CallerContext};

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

/// The MCP handler: every tool call goes to the core for its caller.
#[derive(Clone)]
pub(super) struct SessionTools {
    control: Arc<SessionControl>,
    transport: Transport,
}

/// Where a call's grant, and its caller context when the request sends
/// none, come from.
#[derive(Clone)]
enum Transport {
    /// The local caller's grant for every call, and the context the process
    /// started in.
    Stdio { context: CallerContext },
    /// The grant the HTTP layer put on the request after it checked the
    /// bearer token, which a request without one lacks and is refused.
    Http,
}

impl SessionTools {
    /// The call's caller: its transport's grant, narrowed by the context
    /// the request sends or the transport's own.
    fn caller(&self, context: &RequestContext<RoleServer>) -> Result<Caller, ErrorData> {
        let sent = CallerContext::from_meta(&context.meta)
            .map_err(|error| ErrorData::invalid_params(format!("{error:#}"), None))?;
        let (grant, default) = match &self.transport {
            Transport::Stdio { context } => (Grant::local(), context.clone()),
            Transport::Http => {
                let grant = context
                    .extensions
                    .get::<::http::request::Parts>()
                    .and_then(|parts| parts.extensions.get::<Grant>())
                    .cloned()
                    .ok_or_else(|| {
                        ErrorData::invalid_request("the request carries no token", None)
                    })?;
                (grant, CallerContext::ROOT)
            }
        };
        Ok(Caller::new(grant, &sent.unwrap_or(default)))
    }
}

impl ServerHandler for SessionTools {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_experimental_with(CallerContext::capability())
            .build();
        ServerConfig::new(capabilities)
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
        let caller = self.caller(&context)?;
        let arguments = request.arguments.unwrap_or_default();
        let result = match self.control.call(&caller, action, arguments).await {
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

/// Serves the tools over stdio until the host closes stdin, for the
/// process's caller, in `context` unless a call sends its own.
pub(super) async fn serve_stdio(
    control: Arc<SessionControl>,
    context: CallerContext,
) -> anyhow::Result<()> {
    let tools = SessionTools {
        control,
        transport: Transport::Stdio { context },
    };
    let running = tools.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
