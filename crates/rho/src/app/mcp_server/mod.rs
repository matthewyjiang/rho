//! `rho mcp serve`: the session-control core (`app::session_control`) as an
//! MCP server, over stdio or streamable HTTP (`http_server.rs`).
//!
//! The server is a thin adapter: each MCP tool is one core action, its
//! input schema the action's params, its result the action's JSON. It adds
//! no policy. Over stdio the caller is whoever started the process, with a
//! [`Grant::local`] of the access `--scope` gives; over HTTP each request's
//! bearer token names its grant (`tokens.rs`). The core enforces both.
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
//!
//! Over HTTP the server adds webhook watches (`watch/`): `watch` and
//! `unwatch`, and `notify` on `new` and `prompt`, which have it POST a
//! session's news to a target the owner configured (`webhooks.rs`). Every
//! other call still goes to the core unchanged. A server that does not keep
//! the rho home's watches, because another one does, lists none of them.

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

use rho_protocol::ClientEnvironment;

use super::session_control::{Action, Caller, Grant, SessionControl};
use crate::caller_context::{Access, CallerContext};

mod http_server;
pub(crate) mod token_store;
pub(crate) mod tokens;
pub(crate) mod watch;
pub(crate) mod webhooks;

pub(super) use http_server::{serve_http, HttpOptions};
use watch::{HttpTool, WatchMode, Watches};

/// What the server tells a host about itself.
const INSTRUCTIONS: &str = "Start, drive, and follow rho agent sessions through the rho daemon. \
    `new` starts a session in a workspace with a first prompt; `wait` follows it until it \
    settles or needs a person, returning a cursor to call again while `done` is false; `read` \
    pages through a transcript. A turn that needs an approval or an answer waits for a person in rho: no tool \
    here can answer it.";

/// The MCP handler: every tool call goes to the core for its caller.
#[derive(Clone)]
pub(super) struct SessionTools {
    control: Arc<SessionControl>,
    transport: Transport,
    /// The environment the sessions a call starts or prompts run with, the
    /// call's context set in it: this process's.
    environment: ClientEnvironment,
}

/// Where a call's grant, and its caller context when the request sends
/// none, come from.
#[derive(Clone)]
enum Transport {
    /// The local caller's grant for every call, and the context the process
    /// started in.
    Stdio {
        grant: Grant,
        context: CallerContext,
    },
    /// The grant the HTTP layer put on the request after it checked the
    /// bearer token, which a request without one lacks and is refused; and
    /// whether the server keeps the webhook watches.
    Http { watches: WatchMode },
}

impl Transport {
    /// The watches, when this server keeps them and offers their tools.
    fn watches(&self) -> Option<&Watches> {
        match self {
            Self::Http {
                watches: WatchMode::On(watches),
            } => Some(watches.as_ref()),
            Self::Stdio { .. }
            | Self::Http {
                watches: WatchMode::Off,
            } => None,
        }
    }
}

impl SessionTools {
    /// The call's caller: its transport's grant, narrowed by the context
    /// the request sends or the transport's own.
    fn caller(&self, context: &RequestContext<RoleServer>) -> Result<Caller, ErrorData> {
        let sent = CallerContext::from_meta(&context.meta)
            .map_err(|error| ErrorData::invalid_params(format!("{error:#}"), None))?;
        let (grant, default) = match &self.transport {
            Transport::Stdio { grant, context } => (grant.clone(), context.clone()),
            Transport::Http { .. } => {
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
        Ok(Caller::new(
            grant,
            &sent.unwrap_or(default),
            self.environment.clone(),
        ))
    }
}

impl ServerHandler for SessionTools {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_experimental_with(CallerContext::capability())
            .build();
        let instructions = match self.transport.watches() {
            Some(watches) => format!("{INSTRUCTIONS} {}", watches.instructions()),
            None => INSTRUCTIONS.to_owned(),
        };
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("rho", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let watching = self.transport.watches().is_some();
        let mut tools: Vec<Tool> = Action::ALL
            .into_iter()
            .map(|action| {
                let schema = action.input_schema();
                let schema = if watching {
                    watch::with_notify(action, schema)
                } else {
                    schema
                };
                tool(action, schema)
            })
            .collect();
        if watching {
            tools.extend(watch::watch_tools());
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Refused before anything runs.
        let unknown =
            || ErrorData::invalid_params(format!("no tool named {:?}", request.name), None);
        let arguments = request.arguments.unwrap_or_default();
        let outcome = match &self.transport {
            Transport::Stdio { .. } => {
                let action = Action::from_name(&request.name).ok_or_else(unknown)?;
                let caller = self.caller(&context)?;
                self.control
                    .call(&caller, action, arguments)
                    .await
                    .map_err(|error| error.to_string())
            }
            // Parsed whether or not this server keeps the watches: one that
            // does not answers their tools, and `notify`, that it does not.
            Transport::Http { watches } => {
                let tool = HttpTool::from_name(&request.name).ok_or_else(unknown)?;
                let caller = self.caller(&context)?;
                watches
                    .call(&self.control, &caller, tool, arguments)
                    .await
                    .map_err(|error| error.to_string())
            }
        };
        let result = match outcome {
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value)
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            // Refusals and failures go to the model, which can act on them.
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        Ok(result.into())
    }
}

/// The MCP tool for `action`, taking `schema`.
fn tool(action: Action, schema: serde_json::Map<String, serde_json::Value>) -> Tool {
    let annotations =
        ToolAnnotations::with_title(action.name()).read_only(action.needs() == Access::Read);
    Tool::new(action.name(), action.description(), schema).with_annotations(annotations)
}

/// Serves the tools over stdio until the host closes stdin, for the
/// process's caller with `access`, in `context` unless a call sends its
/// own, the sessions they start running with `environment`.
pub(super) async fn serve_stdio(
    control: Arc<SessionControl>,
    access: Access,
    context: CallerContext,
    environment: ClientEnvironment,
) -> anyhow::Result<()> {
    let tools = SessionTools {
        control,
        transport: Transport::Stdio {
            grant: Grant::local(access),
            context,
        },
        environment,
    };
    let running = tools.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
