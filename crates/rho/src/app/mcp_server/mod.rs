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
//! No tool answers a session's approvals or questions: the server serves
//! the core's actions but `answer` (`tool::served_actions`), whose caller
//! must be the session that started the asker, and an unknown tool name is
//! refused before anything runs.
//!
//! Over HTTP the server adds webhook watches (`watch/`): `watch` and
//! `unwatch`, and `notify` on `new` and `prompt`, which have it POST a
//! session's news to a target the owner configured (`webhooks.rs`). Every
//! other call still goes to the core unchanged. A server that does not keep
//! the rho home's watches, because another one does, lists none of them.
//!
//! A server with remotes is a hub (`remotes/`): every tool takes `server`,
//! and a call for another machine goes to that machine's server. The hub
//! keeps the watches of remote sessions itself, so remotes need no webhook.

use std::sync::Arc;

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, MetaObject, PaginatedRequestParams, ServerCapabilities, ServerConfig,
        Tool, ToolAnnotations,
    },
    service::RequestContext,
    ErrorData, RoleServer, ServerHandler, ServiceExt,
};

use rho_protocol::ClientEnvironment;

use super::session_control::{Action, Caller, Grant, SessionControl};
use crate::caller_context::{Access, CallerContext};

mod http_server;
pub(crate) mod remotes;
pub(crate) mod token_store;
pub(crate) mod tokens;
mod tool;
pub(crate) mod watch;
pub(crate) mod webhooks;

pub(super) use http_server::{serve_http, HttpOptions};
use remotes::{Arrival, Asked, Lasts, Machines, Route, Servers, LASTING};
use tool::ServedTool;
use watch::{WatchMode, Watches};

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
    /// This server, by name, and the remotes it reaches.
    servers: Servers,
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
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let watching = self.transport.watches().is_some();
        let machines = self.servers.load(Arrival::of(&context.meta)).await;
        let mut tools: Vec<Tool> = tool::served_actions()
            .map(|action| {
                let schema = action.input_schema();
                let schema = if watching {
                    watch::with_notify(action, schema)
                } else {
                    schema
                };
                tool(
                    action,
                    machines.with_server(ServedTool::Core(action), schema),
                )
            })
            .collect();
        if watching {
            tools.extend(watch::watch_tools().map(|(served, mut tool)| {
                let schema = machines.with_server(served, (*tool.input_schema).clone());
                tool.input_schema = Arc::new(schema);
                tool
            }));
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Refused before anything runs. Stdio serves no watch tools.
        let tool = match &self.transport {
            Transport::Stdio { .. } => ServedTool::core(&request.name),
            Transport::Http { .. } => ServedTool::from_name(&request.name),
        }
        .ok_or_else(|| {
            ErrorData::invalid_params(format!("no tool named {:?}", request.name), None)
        })?;
        let arrival = Arrival::of(&context.meta);
        let caller = match (self.caller(&context)?, arrival) {
            // A hub's watch asks on nobody's call.
            (caller, Arrival::Forwarded(Asked::ByWatch)) => caller.running_only(),
            (caller, Arrival::Direct | Arrival::Forwarded(Asked::ByCall)) => caller,
        };
        let machines = self.servers.load(arrival).await;
        let arguments = request.arguments.unwrap_or_default();
        let result = match self.dispatch(&caller, &machines, tool, arguments).await {
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value)
                    .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            // Refusals and failures go to the model, which can act on them,
            // and say whether they last, which a hub's watch reads.
            Err((error, lasts)) => {
                let result = CallToolResult::error(vec![ContentBlock::text(error)]);
                match lasts {
                    Lasts::ForGood => {
                        result.with_meta(Some(MetaObject(serde_json::Map::from_iter([(
                            LASTING.to_owned(),
                            serde_json::Value::Bool(true),
                        )]))))
                    }
                    Lasts::ForNow => result,
                }
            }
        };
        Ok(result.into())
    }
}

impl SessionTools {
    /// Runs `tool` for `caller` on the one of `machines` its `arguments`
    /// name.
    async fn dispatch(
        &self,
        caller: &Caller,
        machines: &Machines,
        tool: ServedTool,
        mut arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, (String, Lasts)> {
        let target = match machines
            .route(tool, &mut arguments)
            .map_err(|error| (error.to_string(), error.lasts()))?
        {
            Route::Every => return Ok(machines.list_every(&self.control, caller, arguments).await),
            Route::One(target) => target,
        };
        let mut result = match (&self.transport, tool) {
            (Transport::Http { watches }, tool) => watches
                .call(&self.control, caller, tool, arguments, &target)
                .await
                .map_err(|error| (error.to_string(), error.lasts()))?,
            (Transport::Stdio { .. }, ServedTool::Core(action)) => target
                .call(&self.control, caller, action, arguments)
                .await
                .map_err(|error| (error.to_string(), error.lasts()))?,
            // `call_tool` refused these already.
            (Transport::Stdio { .. }, ServedTool::Watch | ServedTool::Unwatch) => {
                return Err((format!("no tool named {:?}", tool.name()), Lasts::ForGood))
            }
        };
        machines.stamp(&mut result, &target, tool);
        Ok(result)
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
/// own, the sessions they start running with `environment`, as the server
/// called `name`.
pub(super) async fn serve_stdio(
    control: Arc<SessionControl>,
    access: Access,
    context: CallerContext,
    environment: ClientEnvironment,
    name: String,
) -> anyhow::Result<()> {
    let tools = SessionTools {
        control,
        transport: Transport::Stdio {
            grant: Grant::local(access),
            context,
        },
        servers: Servers::new(name),
        environment,
    };
    let running = tools.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
