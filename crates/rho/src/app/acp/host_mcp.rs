//! Converts host-supplied ACP `mcpServers` into session-scoped MCP config.
//!
//! Hosts such as T3 Code hand every session the MCP servers that expose host
//! features (browser preview, orchestration). Rho connects them alongside the
//! servers from its own config. A host entry whose name matches a config
//! server replaces it for that session, because the host's entry carries
//! session-specific credentials.

use std::collections::BTreeMap;

use agent_client_protocol::schema::v1::{McpServer, McpServerHttp, McpServerStdio};

use crate::tools::mcp::{
    config::{
        InvalidMcpServer, McpConfig, McpSamplingPolicy, McpServerConfig, McpToolFilter,
        McpTransport,
    },
    parse_remote_url, validate_identity, validate_literal_headers, validate_stdio_environment,
};

/// Builds the session's host MCP config. Invalid entries land in
/// `invalid_servers` so the MCP report shows them; transports Rho cannot
/// speak are logged and skipped.
pub(super) fn host_mcp_config(servers: &[McpServer]) -> McpConfig {
    let mut config = McpConfig::default();
    for server in servers {
        let (name, converted) = match server {
            McpServer::Stdio(stdio) => (stdio.name.as_str(), stdio_server(stdio)),
            McpServer::Http(http) => (http.name.as_str(), http_server(http)),
            McpServer::Sse(sse) => {
                tracing::warn!(server = %sse.name, "skipping host MCP server: SSE transport is not supported");
                continue;
            }
            // McpServer is non_exhaustive; a transport Rho does not know cannot connect.
            _ => {
                tracing::warn!("skipping host MCP server with an unsupported transport");
                continue;
            }
        };
        let converted =
            validate_identity(name).and_then(|()| converted.map(|server| (name, server)));
        match converted {
            Ok((name, server)) => {
                if config.servers.insert(name.to_string(), server).is_some() {
                    tracing::warn!(server = %name, "host MCP server listed twice; keeping the last entry");
                }
            }
            Err(error) => config.invalid_servers.push(InvalidMcpServer {
                identity: name.to_string(),
                error: format!("{error:#}"),
            }),
        }
    }
    config
}

fn stdio_server(server: &McpServerStdio) -> anyhow::Result<McpServerConfig> {
    let command = server.command.to_string_lossy().into_owned();
    if command.trim().is_empty() {
        anyhow::bail!("stdio command must not be empty");
    }
    let mut env = BTreeMap::new();
    for variable in &server.env {
        if env
            .insert(variable.name.clone(), variable.value.clone())
            .is_some()
        {
            anyhow::bail!("stdio env repeats child variable `{}`", variable.name);
        }
    }
    let env_from_env = BTreeMap::new();
    validate_stdio_environment(&env, &env_from_env)?;
    Ok(host_server(McpTransport::Stdio {
        command,
        args: server.args.clone(),
        cwd: None,
        env,
        env_from_env,
    }))
}

fn http_server(server: &McpServerHttp) -> anyhow::Result<McpServerConfig> {
    // Host entries get the same transport rule as plugin servers: HTTPS, or
    // plain HTTP only to loopback. Only the user's own config may opt out.
    parse_remote_url(&server.url)?;
    let mut headers = BTreeMap::new();
    for header in &server.headers {
        if headers
            .insert(header.name.clone(), header.value.clone())
            .is_some()
        {
            anyhow::bail!("header `{}` is listed twice", header.name);
        }
    }
    validate_literal_headers(&headers)?;
    Ok(host_server(McpTransport::StreamableHttp {
        url: server.url.clone(),
        headers,
        headers_from_env: BTreeMap::new(),
        oauth: None,
        allow_insecure_http: false,
    }))
}

fn host_server(transport: McpTransport) -> McpServerConfig {
    McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        log_level: None,
        // A host cannot opt a server into spending the user's model tokens.
        sampling: McpSamplingPolicy::Deny,
        transport,
        filesystem: None,
    }
}

#[cfg(test)]
#[path = "host_mcp_tests.rs"]
mod tests;
