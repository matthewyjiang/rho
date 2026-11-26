//! The MCP servers a session would configure, found without starting any:
//! what `rho mcp list` prints and `rho/mcp/list` answers.

use std::path::Path;

use rho_protocol::{ConfiguredMcpServer, McpServerSource, McpTransport};

use crate::{
    tools::mcp::{config::McpConfig, McpLoadMode, McpSessionReport, McpTransportSummary},
    workspace::ProjectTrusts,
};

/// The servers of the config and of the plugins that apply, kept apart so
/// each server's source stays known.
pub(crate) struct ConfiguredMcp {
    config: McpConfig,
    plugins: McpConfig,
}

impl ConfiguredMcp {
    /// `config`'s servers and those of the plugins a session opened in `cwd`
    /// loads under `trust`; no plugin's without a `cwd`.
    pub(crate) fn discover(config: &McpConfig, cwd: Option<&Path>, trust: ProjectTrusts) -> Self {
        let plugins = cwd
            .map(|cwd| {
                let discovery =
                    crate::plugins::discover_mcp(cwd, crate::paths::home_dir().as_deref(), trust);
                crate::plugins::log(&discovery.report);
                discovery.mcp
            })
            .unwrap_or_default();
        Self {
            config: config.clone(),
            plugins,
        }
    }

    /// Every server, the plugins' merged with the config's as a session
    /// merges them.
    pub(crate) fn merged(self) -> McpConfig {
        let mut merged = self.config;
        merged.merge(self.plugins);
        merged
    }

    /// Each server as `rho/mcp/list` lists it, by identity, as `rho mcp
    /// list` orders them.
    pub(crate) fn listing(&self) -> Vec<ConfiguredMcpServer> {
        let mut servers = listed(&self.config, |_| McpServerSource::Config);
        servers.extend(listed(&self.plugins, |identity| {
            // A plugin's servers are named `<plugin>/<server>`
            // (`plugins::discover_components`).
            let plugin = identity
                .split_once('/')
                .map_or(identity, |(plugin, _)| plugin);
            McpServerSource::Plugin {
                plugin: plugin.to_owned(),
            }
        }));
        servers.sort_by(|left, right| left.identity.cmp(&right.identity));
        servers
    }
}

/// `config`'s servers as the unstarted inventory reports them, each from
/// the source `source` names for its identity.
fn listed(
    config: &McpConfig,
    source: impl Fn(&str) -> McpServerSource,
) -> Vec<ConfiguredMcpServer> {
    McpSessionReport::from_config_unloaded(config, McpLoadMode::Native)
        .servers
        .into_iter()
        .map(|server| {
            let transport = server.transport.as_ref().map(|transport| match transport {
                McpTransportSummary::Stdio { command, args } => McpTransport::Stdio {
                    command: command.clone(),
                    args: args.clone(),
                },
                McpTransportSummary::StreamableHttp { url } => {
                    McpTransport::StreamableHttp { url: url.clone() }
                }
            });
            ConfiguredMcpServer::new(
                server.identity.clone(),
                source(&server.identity),
                server.enabled(),
                transport,
                server.error().map(str::to_owned),
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "configured_mcp_tests.rs"]
mod tests;
