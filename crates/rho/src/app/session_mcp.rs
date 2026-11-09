//! The session's MCP inventory, shared by in-process and daemon frontends.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use rho_protocol::{SessionMcpInventory, SessionMcpMode, SessionMcpServer, SessionMcpStatus};

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
}

#[cfg(unix)]
impl SessionMcp {
    pub(super) fn of(runtime: &SessionRuntime) -> Self {
        Self {
            report: runtime.tools().mcp_report().clone(),
            catalog: runtime.tools().mcp_catalog().clone(),
            config_path: runtime.config_path().to_path_buf(),
        }
    }

    pub(super) fn snapshot(&self) -> SessionMcpInventory {
        session_mcp_inventory(&self.report, &self.catalog, &self.config_path)
    }
}

/// Build the same inventory for either host, including live catalog details.
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
