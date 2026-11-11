//! The session's MCP inventory, shared by in-process and daemon frontends.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
use rho_protocol::{
    SessionMcpCatalog, SessionMcpPrompt, SessionMcpPromptArgument, SessionMcpResource,
};
use rho_protocol::{SessionMcpInventory, SessionMcpMode, SessionMcpServer, SessionMcpStatus};

#[cfg(unix)]
use crate::tools::mcp::McpCompletionSupport;
use crate::tools::mcp::{McpCatalog, McpLoadMode, McpServerStatus, McpSessionReport};

#[cfg(unix)]
use super::session_runtime::SessionRuntime;

/// Handles to a session's inventory, readable while its runtime runs a turn.
/// Catalog and live server facts remain shared with MCP maintenance tasks.
#[cfg(unix)]
#[derive(Clone)]
pub(super) struct SessionMcp {
    report: McpSessionReport,
    catalog: McpCatalog,
    config_path: PathBuf,
    /// Reads `true` once the session's deferred tool sources loaded;
    /// `None` when it deferred none, or runs with them.
    tool_sources: Option<tokio::sync::watch::Receiver<bool>>,
}

#[cfg(unix)]
impl SessionMcp {
    pub(super) fn of(runtime: &SessionRuntime) -> Self {
        Self {
            report: runtime.tools().mcp_report().clone(),
            catalog: runtime.tools().mcp_catalog().clone(),
            config_path: runtime.config_path().to_path_buf(),
            tool_sources: runtime.tool_sources_progress(),
        }
    }

    pub(super) fn snapshot(&self) -> SessionMcpInventory {
        SessionMcpInventory {
            tool_sources_loading: self
                .tool_sources
                .as_ref()
                .is_some_and(|loaded| !*loaded.borrow()),
            ..session_mcp_inventory(&self.report, &self.catalog, &self.config_path)
        }
    }

    /// The session's connected servers, to fetch from: a resource to read,
    /// or values for a prompt argument.
    pub(super) fn servers(&self) -> &McpCatalog {
        &self.catalog
    }

    /// What those servers offer now, as a client's composer lists it.
    pub(super) fn offered(&self) -> SessionMcpCatalog {
        SessionMcpCatalog {
            prompts: self
                .catalog
                .prompts()
                .into_iter()
                .map(|prompt| SessionMcpPrompt {
                    completes_arguments: match self.catalog.completion_support(&prompt.server) {
                        McpCompletionSupport::Declared => true,
                        McpCompletionSupport::Absent => false,
                    },
                    server: prompt.server,
                    name: prompt.name,
                    title: prompt.title,
                    description: prompt.description,
                    arguments: prompt
                        .arguments
                        .into_iter()
                        .map(|argument| SessionMcpPromptArgument {
                            name: argument.name,
                            description: argument.description,
                            required: argument.required,
                        })
                        .collect(),
                })
                .collect(),
            resources: self
                .catalog
                .resources()
                .into_iter()
                .map(|resource| SessionMcpResource {
                    server: resource.server,
                    uri: resource.uri,
                    name: resource.name,
                    title: resource.title,
                    description: resource.description,
                    mime_type: resource.mime_type,
                    templated: resource.templated,
                })
                .collect(),
        }
    }
}

/// Build the same inventory for either host, including live catalog details.
/// `tool_sources_loading` is the daemon's to set (see
/// `SessionMcp::snapshot`): the in-process TUI asks its runtime instead.
pub(crate) fn session_mcp_inventory(
    report: &McpSessionReport,
    catalog: &McpCatalog,
    config_path: &Path,
) -> SessionMcpInventory {
    let prompts = catalog.prompts();
    let resources = catalog.resources();
    SessionMcpInventory {
        config_path: config_path.to_path_buf(),
        mode: match report.mode {
            McpLoadMode::Native => SessionMcpMode::Native,
            McpLoadMode::UnsupportedAgent => SessionMcpMode::UnsupportedAgent,
            McpLoadMode::ToolsDisabled => SessionMcpMode::ToolsDisabled,
        },
        tool_sources_loading: false,
        servers: report
            .servers
            .iter()
            .map(|server| {
                let mut detail = server.detail_text();
                let prompts: Vec<_> = prompts
                    .iter()
                    .filter(|prompt| prompt.server == server.identity)
                    .map(|prompt| format!("/{}", prompt.command_name()))
                    .collect();
                let resources = resources
                    .iter()
                    .filter(|resource| resource.server == server.identity)
                    .count();
                if !prompts.is_empty() {
                    detail.push_str(&format!(
                        "\n{} prompt(s): {}",
                        prompts.len(),
                        prompts.join(", ")
                    ));
                }
                if resources > 0 {
                    detail.push_str(&format!("\n{resources} resource(s), available with @"));
                }
                SessionMcpServer {
                    identity: server.identity.clone(),
                    enabled: server.enabled(),
                    status: match server.status() {
                        McpServerStatus::Connected => SessionMcpStatus::Connected,
                        McpServerStatus::Connecting => SessionMcpStatus::Connecting,
                        McpServerStatus::Disabled => SessionMcpStatus::Disabled,
                        McpServerStatus::InvalidConfig => SessionMcpStatus::InvalidConfig,
                        McpServerStatus::Failed => SessionMcpStatus::Failed,
                        McpServerStatus::TimedOut => SessionMcpStatus::TimedOut,
                        McpServerStatus::NotLoaded => SessionMcpStatus::NotLoaded,
                    },
                    tool_count: server.tool_count() as u64,
                    detail,
                }
            })
            .collect(),
    }
}
