//! The MCP servers the daemon's configuration sets up, read without a
//! session: what `rho mcp list` prints.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ClientEnvironment;

/// List the MCP servers a session would configure, without loading a
/// session or starting a server: those of the daemon's `config.toml`, then
/// those of the agent plugins that apply, as `rho mcp list` lists them. A
/// server whose configuration is invalid is listed with its error. A
/// `config.toml` that does not load is [`crate::ErrorCode::InternalError`],
/// naming each problem, which [`super::CONFIG_GET`] also reports. Each
/// request reads the files again. A daemon that predates it answers
/// [`crate::ErrorCode::MethodNotFound`].
pub const MCP_LIST: &str = "rho/mcp/list";

/// Params of [`MCP_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpListParams {
    /// The directory a session would open in, whose project's plugins
    /// contribute servers. Absent, the daemon user's home directory, which
    /// leaves only the user's plugins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The asking client's environment, whose `RHO_TRUST_PROJECT_PLUGINS`
    /// decides whether the project's plugins count, as for a session that
    /// client opens. Absent, they do not.
    #[serde(default, skip_serializing_if = "ClientEnvironment::is_empty")]
    pub environment: ClientEnvironment,
}

impl McpListParams {
    /// Builds it from each of its fields.
    pub fn new(workspace: Option<PathBuf>, environment: ClientEnvironment) -> Self {
        Self {
            workspace,
            environment,
        }
    }
}

/// Result of [`MCP_LIST`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct McpListResponse {
    /// The daemon's `config.toml`, whose `[mcp.servers]` table configures
    /// the servers whose source is [`McpServerSource::Config`].
    pub config_path: PathBuf,
    /// Every configured server, by identity, including disabled and invalid
    /// ones.
    pub servers: Vec<ConfiguredMcpServer>,
}

impl McpListResponse {
    /// Builds it from each of its fields.
    pub fn new(config_path: PathBuf, servers: Vec<ConfiguredMcpServer>) -> Self {
        Self {
            config_path,
            servers,
        }
    }
}

/// One configured MCP server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ConfiguredMcpServer {
    /// Qualified server identity: `<plugin>/<server>` for a plugin's, as
    /// [`super::SessionMcpServer::identity`].
    pub identity: String,
    pub source: McpServerSource,
    /// Whether a session starts it: false when its configuration disables
    /// it or is invalid.
    pub enabled: bool,
    /// How a session reaches it; absent when its configuration is invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<McpTransport>,
    /// Why its configuration is invalid, when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ConfiguredMcpServer {
    /// Builds it from each of its fields.
    pub fn new(
        identity: String,
        source: McpServerSource,
        enabled: bool,
        transport: Option<McpTransport>,
        error: Option<String>,
    ) -> Self {
        Self {
            identity,
            source,
            enabled,
            transport,
            error,
        }
    }
}

/// Where a server's configuration comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpServerSource {
    /// The `[mcp.servers]` table of [`McpListResponse::config_path`].
    Config,
    /// The `mcp.json` of the agent plugin named `plugin`.
    Plugin { plugin: String },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// How a session reaches a server, as its configuration says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpTransport {
    /// A program the session starts, and its arguments.
    Stdio { command: String, args: Vec<String> },
    /// A remote server at `url`.
    StreamableHttp { url: String },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
