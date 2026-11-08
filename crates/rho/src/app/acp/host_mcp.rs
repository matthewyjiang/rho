//! Maps host-supplied ACP `mcpServers` onto the daemon protocol's MCP
//! servers, which `app::host_mcp` turns into session-scoped MCP config.
//! Transports Rho cannot speak are logged and skipped.

use agent_client_protocol::schema::v1::McpServer as AcpMcpServer;
use rho_protocol::{McpServer, SecretMap};

/// The servers ACP `servers` name, in order, but those whose transport Rho
/// cannot speak.
pub(super) fn protocol_servers(servers: &[AcpMcpServer]) -> Vec<McpServer> {
    servers.iter().filter_map(protocol_server).collect()
}

fn protocol_server(server: &AcpMcpServer) -> Option<McpServer> {
    match server {
        AcpMcpServer::Stdio(stdio) => Some(McpServer::Stdio {
            name: stdio.name.clone(),
            command: stdio.command.clone(),
            args: stdio.args.clone(),
            env: by_name(
                &stdio.name,
                stdio
                    .env
                    .iter()
                    .map(|variable| (variable.name.as_str(), variable.value.as_str())),
            ),
        }),
        AcpMcpServer::Http(http) => Some(McpServer::Http {
            name: http.name.clone(),
            url: http.url.clone(),
            headers: by_name(
                &http.name,
                http.headers
                    .iter()
                    .map(|header| (header.name.as_str(), header.value.as_str())),
            ),
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

/// Server `server`'s environment variables or headers, an ACP list, by
/// name. When a name repeats, the last value wins.
fn by_name<'a>(server: &str, listed: impl Iterator<Item = (&'a str, &'a str)>) -> SecretMap {
    let mut values = std::collections::BTreeMap::new();
    for (name, value) in listed {
        if values.insert(name.to_owned(), value.to_owned()).is_some() {
            tracing::warn!(
                server,
                entry = name,
                "host MCP server lists a name twice; keeping the last value"
            );
        }
    }
    values.into_iter().collect()
}
