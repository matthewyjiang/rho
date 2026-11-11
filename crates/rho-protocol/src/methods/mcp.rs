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
