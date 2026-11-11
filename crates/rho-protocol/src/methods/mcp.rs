//! On-demand inventory of the MCP servers owned by a loaded session.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// Read a loaded session's MCP inventory without waiting for its active turn.
/// Does not load a session or change its configuration. An unloaded session
/// returns [`crate::ErrorCode::SessionNotLoaded`]. Reissue to refresh live
/// keepalive, tool-change, prompt and resource details.
pub const SESSION_MCP: &str = "rho/session/mcp";

/// Params of [`SESSION_MCP`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpParams {
    /// The loaded session whose servers to inspect.
    pub session: SessionId,
}

/// Result of [`SESSION_MCP`]: a point-in-time inventory, not a subscription.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpInventory {
    /// The configuration file the session was opened with, not the client's.
    pub config_path: PathBuf,
    /// Why the session did or did not load native MCP servers.
    pub mode: SessionMcpMode,
    /// Configured servers in inventory order, including disabled and failed ones.
    pub servers: Vec<SessionMcpServer>,
    /// Whether the session still loads the tool sources it opened with, its
    /// MCP servers and OpenAPI sources. Its prompts wait until it has. No
    /// server reads [`SessionMcpStatus::Connecting`] for an OpenAPI source,
    /// so this is the only sign that one loads.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tool_sources_loading: bool,
}

/// How a session treated its MCP configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionMcpMode {
    /// The session hosts native MCP servers.
    Native,
    /// The session's agent runtime does not host native MCP tools.
    UnsupportedAgent,
    /// The session was opened without tools.
    ToolsDisabled,
    /// A mode a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// One configured MCP server and its session-owned status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpServer {
    /// Qualified server identity, including its plugin namespace when present.
    pub identity: String,
    /// Whether the session enabled this server.
    pub enabled: bool,
    /// Startup status; subsequent reachability changes are described in `detail`.
    pub status: SessionMcpStatus,
    /// Number of tools exported to this session's model.
    pub tool_count: u64,
    /// Human-readable transport, errors, exported tools, live changes, prompts
    /// and resources. Presentation only: clients must not parse this text.
    pub detail: String,
}

/// A server's startup outcome in a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionMcpStatus {
    /// Connected at startup; see the server's detail for live reachability.
    Connected,
    /// Startup has not finished.
    Connecting,
    /// Disabled by configuration.
    Disabled,
    /// Invalid server configuration.
    InvalidConfig,
    /// Startup failed.
    Failed,
    /// Startup exceeded its configured budget.
    TimedOut,
    /// Configured but not loaded by this session.
    NotLoaded,
    /// A status a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Read the prompts and resources a loaded session's MCP servers offer now,
/// without waiting for its active turn: what a composer completes an `@`
/// mention and a prompt command's arguments from. Does not load a session.
/// An unloaded session returns [`crate::ErrorCode::SessionNotLoaded`]. A
/// point-in-time listing: reissue it to see what a server added or removed.
pub const SESSION_MCP_CATALOG: &str = "rho/session/mcp/catalog";

/// Params of [`SESSION_MCP_CATALOG`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpCatalogParams {
    /// The loaded session whose servers to list.
    pub session: SessionId,
}

/// Result of [`SESSION_MCP_CATALOG`], each list ordered by server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpCatalog {
    pub prompts: Vec<SessionMcpPrompt>,
    pub resources: Vec<SessionMcpResource>,
}

/// One prompt a server offers, which the session runs as the command
/// `mcp:<server>:<name>` (see [`crate::SessionCommand`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpPrompt {
    /// Qualified identity of the server that offers it.
    pub server: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The arguments it declares, in the server's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<SessionMcpPromptArgument>,
    /// Whether its server suggests values for those arguments, so
    /// [`SESSION_MCP_COMPLETE`] is worth a request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub completes_arguments: bool,
}

/// One argument an MCP prompt declares.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpPromptArgument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// One resource a server offers: a concrete URI, or a URI template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpResource {
    /// Qualified identity of the server that offers it.
    pub server: String,
    /// The resource's URI, or its RFC 6570 URI template when `templated`.
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// The URI has placeholders to fill in before
    /// [`SESSION_MCP_RESOURCE`] can read it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub templated: bool,
}

/// Read one resource from a loaded session's MCP server, without waiting
/// for the session's active turn. Does not load a session. A server the
/// session has not connected is [`crate::ErrorCode::InvalidParams`]; a read
/// the server refuses or fails, or one too large for a protocol line, is
/// [`crate::ErrorCode::InternalError`] with the reason as its message.
pub const SESSION_MCP_RESOURCE: &str = "rho/session/mcp/resource";

/// Params of [`SESSION_MCP_RESOURCE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpResourceParams {
    pub session: SessionId,
    /// Qualified identity of the server, as [`SessionMcpResource::server`].
    pub server: String,
    /// A concrete URI: a listed one, or a template with its placeholders
    /// filled in.
    pub uri: String,
}

/// Result of [`SESSION_MCP_RESOURCE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpResourceResponse {
    /// The bodies the server sent, in its order. One read may return several.
    pub contents: Vec<SessionMcpResourceContent>,
}

/// One body of a resource, as its server sent it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionMcpResourceContent {
    Text {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// The bytes, base64-encoded.
        blob: String,
    },
    /// A body of a kind this protocol does not carry: one the daemon's MCP
    /// revision does not define, or a kind a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Ask a loaded session's MCP server for values of one prompt argument,
/// without waiting for the session's active turn. Does not load a session.
/// Suggestions are help, so a server that offers none, does not complete
/// arguments, or fails the request answers with no values and no error.
pub const SESSION_MCP_COMPLETE: &str = "rho/session/mcp/complete";

/// Params of [`SESSION_MCP_COMPLETE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpCompleteParams {
    pub session: SessionId,
    /// Qualified identity of the server, as [`SessionMcpPrompt::server`].
    pub server: String,
    /// The prompt's name, as [`SessionMcpPrompt::name`].
    pub prompt: String,
    /// The argument's name, as [`SessionMcpPromptArgument::name`].
    pub argument: String,
    /// What is typed of the argument's value so far; may be empty.
    pub value: String,
}

/// Result of [`SESSION_MCP_COMPLETE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionMcpCompleteResponse {
    /// Suggested values, best first, as the server ordered them.
    pub values: Vec<String>,
}
