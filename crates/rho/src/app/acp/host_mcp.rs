//! Maps host-supplied ACP `mcpServers` onto the daemon protocol's MCP
//! servers, which `app::host_mcp` turns into session-scoped MCP
//! config. Transports Rho cannot speak are logged and skipped.

use agent_client_protocol::schema::v1::McpServer as AcpMcpServer;
use rho_protocol::{EnvVariable, HttpHeader, McpServer};

use crate::{app::host_mcp, tools::mcp::config::McpConfig};

/// The session's host MCP config for ACP `servers`.
pub(super) fn host_mcp_config(servers: &[AcpMcpServer]) -> McpConfig {
    let servers: Vec<McpServer> = servers.iter().filter_map(protocol_server).collect();
    host_mcp::host_mcp_config(&servers)
}

fn protocol_server(server: &AcpMcpServer) -> Option<McpServer> {
    match server {
        AcpMcpServer::Stdio(stdio) => Some(McpServer::Stdio {
            name: stdio.name.clone(),
            command: stdio.command.clone(),
            args: stdio.args.clone(),
            env: stdio
                .env
                .iter()
                .map(|variable| EnvVariable {
                    name: variable.name.clone(),
                    value: variable.value.clone(),
                })
                .collect(),
        }),
        AcpMcpServer::Http(http) => Some(McpServer::Http {
            name: http.name.clone(),
            url: http.url.clone(),
            headers: http
                .headers
                .iter()
                .map(|header| HttpHeader {
                    name: header.name.clone(),
                    value: header.value.clone(),
                })
                .collect(),
        }),
        AcpMcpServer::Sse(sse) => {
            tracing::warn!(server = %sse.name, "skipping host MCP server: SSE transport is not supported");
            None
        }
        // McpServer is non_exhaustive; a transport Rho does not know cannot connect.
        _ => {
            tracing::warn!("skipping host MCP server with an unsupported transport");
            None
        }
    }
}
