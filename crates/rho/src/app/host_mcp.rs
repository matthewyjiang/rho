//! Converts the MCP servers a client provides for a session into
//! session-scoped MCP config: a daemon client's
//! `SessionSetup::mcp_servers`, and an ACP host's `mcpServers`, which
//! `rho acp` maps onto the same servers first (`acp::requests`).
//!
//! A client provides servers for features of its own, such as an editor's
//! browser preview. Rho connects them alongside the servers from its own
//! config; the session runtime overlays them, so a client's entry replaces
//! a config server of the same name, even when it is invalid
//! (`app::session_runtime::session_config`).

use std::collections::BTreeMap;

use rho_protocol::{McpServer, SecretMap};

use crate::tools::mcp::{
    config::{
        InvalidMcpServer, McpConfig, McpSamplingPolicy, McpServerConfig, McpToolFilter,
        McpTransport,
    },
    parse_remote_url, validate_identity, validate_literal_headers, validate_stdio_environment,
};

/// Builds the session's host MCP config. When a name repeats, the last entry
/// wins whether it is valid or not. Invalid entries land in `invalid_servers`
/// so the MCP report shows them.
pub(crate) fn host_mcp_config(servers: &[McpServer]) -> McpConfig {
    let mut config = McpConfig::default();
    for server in servers {
        let (name, converted) = match server {
            McpServer::Stdio {
                name,
                command,
                args,
                env,
            } => (name.as_str(), stdio_server(command, args, env)),
            McpServer::Http { name, url, headers } => (name.as_str(), http_server(url, headers)),
            // A transport of a later protocol minor fails to decode before
            // it gets here; the arm keeps the match open to new variants.
            _ => {
                tracing::warn!("skipping host MCP server with an unsupported transport");
                continue;
            }
        };
        let converted =
            validate_identity(name).and_then(|()| converted.map(|server| (name, server)));
        let replaced_valid = config.servers.remove(name).is_some();
        let before = config.invalid_servers.len();
        config
            .invalid_servers
            .retain(|invalid| invalid.identity != name);
        if replaced_valid || config.invalid_servers.len() != before {
            tracing::warn!(server = %name, "host MCP server listed twice; keeping the last entry");
        }
        match converted {
            Ok((name, server)) => {
                config.servers.insert(name.to_string(), server);
            }
            Err(error) => config.invalid_servers.push(InvalidMcpServer {
                identity: name.to_string(),
                error: format!("{error:#}"),
            }),
        }
    }
    config
}

fn stdio_server(
    command: &std::path::Path,
    args: &[String],
    env: &SecretMap,
) -> anyhow::Result<McpServerConfig> {
    let command = command.to_string_lossy().into_owned();
    if command.trim().is_empty() {
        anyhow::bail!("stdio command must not be empty");
    }
    let env = owned(env);
    let env_from_env = BTreeMap::new();
    validate_stdio_environment(&env, &env_from_env)?;
    Ok(host_server(McpTransport::Stdio {
        command,
        args: args.to_vec(),
        cwd: None,
        env,
        env_from_env,
    }))
}

fn http_server(url: &str, headers: &SecretMap) -> anyhow::Result<McpServerConfig> {
    // Client entries get the same transport rule as plugin servers: HTTPS,
    // or plain HTTP only to loopback. Only the user's own config may opt
    // out.
    parse_remote_url(url)?;
    let headers = owned(headers);
    validate_literal_headers(&headers)?;
    Ok(host_server(McpTransport::StreamableHttp {
        url: url.to_owned(),
        headers,
        headers_from_env: BTreeMap::new(),
        oauth: None,
        allow_insecure_http: false,
    }))
}

/// `values` as the config's map of them, by name.
fn owned(values: &SecretMap) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

fn host_server(transport: McpTransport) -> McpServerConfig {
    McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        log_level: None,
        // A client cannot opt a server into spending the user's model tokens.
        sampling: McpSamplingPolicy::Deny,
        transport,
        filesystem: None,
    }
}

#[cfg(test)]
#[path = "host_mcp_tests.rs"]
mod tests;
