//! Native Model Context Protocol client.
//!
//! Configuration is the trust boundary: enabling a server is permission to
//! start it, discover its tools, and expose them for the session. Everything
//! below is per-session mechanics on top of that decision.

use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use rho_sdk::tool::Tool;

use super::sdk_registry::ToolBundle;
use config::{McpConfig, McpServerConfig};

pub(crate) mod client;
pub(crate) mod config;
pub(crate) mod definition;
pub(crate) mod progress;
pub(crate) mod report;
pub(crate) mod result;
pub(crate) mod roots;
pub(crate) mod session;
pub(crate) mod tool;
pub(crate) mod validate;

pub(crate) use report::{
    McpLoadMode, McpServerReport, McpServerStatus, McpSessionReport, McpToolReport,
    McpTransportSummary,
};
pub(crate) use roots::McpRoots;
pub(crate) use validate::{
    parse_remote_url, validate_environment_header_names, validate_identity,
    validate_literal_headers, validate_stdio_environment,
};

use definition::McpToolDefinition;
use session::{ConnectResult, ConnectedServer, McpSession, SessionMaintenance};
use tool::{namespaced_tool_name, McpTool, McpToolSlot};

/// Whether this session should connect MCP servers or only inventory config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpSessionPlan {
    /// Native runtime with tools: connect enabled servers.
    Connect,
    /// Emit config inventory without starting transports.
    Inventory(McpLoadMode),
}

/// Session-scoped inputs every connected server shares.
#[derive(Clone, Debug)]
pub(crate) struct McpSessionOptions {
    pub(crate) max_output_bytes: usize,
    /// Filesystem roots advertised through `roots/list`.
    pub(crate) roots: McpRoots,
}

impl McpSessionOptions {
    pub(crate) fn new(max_output_bytes: usize, roots: McpRoots) -> Self {
        Self {
            max_output_bytes: max_output_bytes.max(1),
            roots,
        }
    }
}

pub(crate) struct McpConnectOutcome {
    pub(crate) report: McpSessionReport,
    pub(crate) bundle: Option<McpBundle>,
}

impl McpConnectOutcome {
    /// Run the session plan against config: connect or inventory-only.
    pub(crate) async fn run(
        plan: McpSessionPlan,
        config: &McpConfig,
        options: McpSessionOptions,
    ) -> Self {
        match plan {
            McpSessionPlan::Connect => McpBundle::connect(config, options).await,
            McpSessionPlan::Inventory(mode) => Self {
                report: McpSessionReport::from_config_unloaded(config, mode),
                bundle: None,
            },
        }
    }
}

pub(crate) struct McpBundle {
    tools: Vec<Arc<dyn Tool>>,
    /// Taken once during shutdown; tools hold independent peer handles.
    sessions: tokio::sync::Mutex<Vec<McpSession>>,
    /// Per-session maintenance tasks, aborted before the sessions close so a
    /// refresh cannot race teardown.
    maintenance: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl McpBundle {
    /// Connect enabled servers in parallel and discover their tools. Always
    /// returns a structured inventory. The no-enabled-server path exits before
    /// allocating a transport, client, task, or bundle.
    pub(crate) async fn connect(
        config: &McpConfig,
        options: McpSessionOptions,
    ) -> McpConnectOutcome {
        let mut servers = Vec::with_capacity(config.servers.len() + config.invalid_servers.len());

        for invalid in &config.invalid_servers {
            tracing::warn!(
                server = %invalid.identity,
                error = %invalid.error,
                "ignoring invalid MCP server configuration"
            );
            servers.push(McpServerReport::invalid(
                invalid.identity.clone(),
                invalid.error.clone(),
            ));
        }

        for (identity, server) in &config.servers {
            if !server.enabled {
                servers.push(McpServerReport::disabled(identity.clone(), server));
            }
        }

        if !config.has_enabled_servers() {
            servers.sort_by(|left, right| left.identity.cmp(&right.identity));
            return McpConnectOutcome {
                report: McpSessionReport {
                    mode: McpLoadMode::Native,
                    servers,
                },
                bundle: None,
            };
        }

        #[cfg(test)]
        MCP_RUNTIME_CONSTRUCTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let connect_jobs = config
            .servers
            .iter()
            .filter(|(_, server)| server.enabled)
            .map(|(identity, server)| {
                let identity = identity.clone();
                let server = server.clone();
                let roots = options.roots.clone();
                async move {
                    let transport = McpTransportSummary::from_server(&server);
                    let result = session::connect_server_bounded(&identity, &server, &roots).await;
                    (identity, server, transport, result)
                }
            });
        // BTreeMap iteration order is preserved by join_all input order.
        let connect_results = futures_util::future::join_all(connect_jobs).await;

        let mut bundle = McpBundleBuilder::new(options.max_output_bytes);
        for (identity, server, transport, result) in connect_results {
            let connected = match result {
                ConnectResult::Ready(connected) => connected,
                ConnectResult::Failed { error } => {
                    tracing::warn!(server = %identity, error = %error, "MCP server failed to initialize");
                    servers.push(McpServerReport::failed(
                        identity,
                        transport,
                        error.to_string(),
                    ));
                    continue;
                }
                ConnectResult::TimedOut => {
                    tracing::warn!(
                        server = %identity,
                        limit_seconds = session::MCP_SERVER_STARTUP_BUDGET.as_secs(),
                        "MCP server exceeded its startup budget"
                    );
                    servers.push(McpServerReport::timed_out(
                        identity,
                        transport,
                        session::MCP_SERVER_STARTUP_BUDGET.as_secs(),
                    ));
                    continue;
                }
            };
            servers.push(bundle.register(identity, server, transport, *connected));
        }

        servers.sort_by(|left, right| left.identity.cmp(&right.identity));
        McpConnectOutcome {
            report: McpSessionReport {
                mode: McpLoadMode::Native,
                servers,
            },
            bundle: bundle.build(),
        }
    }

    /// Close live MCP sessions. Used by CLI inspect after printing inventory.
    pub(crate) async fn close(&self) {
        for task in std::mem::take(&mut *self.maintenance.lock().await) {
            task.abort();
        }
        let sessions = {
            let mut guard = self.sessions.lock().await;
            std::mem::take(&mut *guard)
        };
        let close_jobs = sessions.into_iter().map(session::close_session);
        futures_util::future::join_all(close_jobs).await;
    }
}

impl ToolBundle for McpBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.close())
    }
}

/// Accumulates exported tools, sessions, and maintenance tasks while servers
/// finish connecting, so `connect` stays a readable pass over the results.
struct McpBundleBuilder {
    max_output_bytes: usize,
    tools: Vec<Arc<dyn Tool>>,
    sessions: Vec<McpSession>,
    maintenance: Vec<tokio::task::JoinHandle<()>>,
    registered_names: HashSet<String>,
}

impl McpBundleBuilder {
    fn new(max_output_bytes: usize) -> Self {
        Self {
            max_output_bytes,
            tools: Vec::new(),
            sessions: Vec::new(),
            maintenance: Vec::new(),
            registered_names: HashSet::new(),
        }
    }

    fn register(
        &mut self,
        identity: String,
        server: McpServerConfig,
        transport: McpTransportSummary,
        connected: ConnectedServer,
    ) -> McpServerReport {
        let ConnectedServer {
            session,
            discovered,
            instructions,
            progress,
            events,
        } = connected;
        let mut exported = Vec::new();
        let mut slots = BTreeMap::new();
        let mut filtered_out_count = 0usize;
        let mut collision_skipped_count = 0usize;
        for remote in discovered {
            let remote_name = remote.name.to_string();
            if !server.tools.includes(&remote_name) {
                filtered_out_count += 1;
                continue;
            }
            let name = namespaced_tool_name(&identity, &remote_name);
            if !self.registered_names.insert(name.clone()) {
                tracing::warn!(server = %identity, tool = %remote_name, exported = %name, "MCP tool name collision; ignoring tool");
                collision_skipped_count += 1;
                continue;
            }
            let slot = Arc::new(McpToolSlot::new(McpToolDefinition::from_remote(
                &identity,
                &remote_name,
                &remote,
            )));
            slots.insert(remote_name.clone(), Arc::clone(&slot));
            self.tools.push(Arc::new(McpTool {
                slot,
                identity: identity.clone(),
                remote_name: remote_name.clone(),
                peer: session.peer().clone(),
                progress: progress.clone(),
                transport: server.transport.clone(),
                max_output_bytes: self.max_output_bytes,
            }));
            exported.push(McpToolReport {
                remote_name,
                exported_name: name,
            });
        }

        let live = report::McpLiveServerState::default();
        self.maintenance
            .push(tokio::spawn(session::maintain_session(
                SessionMaintenance {
                    identity: identity.clone(),
                    peer: session.peer().clone(),
                    server,
                    slots,
                    live: live.clone(),
                    events,
                },
            )));
        self.sessions.push(session);
        McpServerReport::connected(report::ConnectedServerReport {
            identity,
            transport,
            tools: exported,
            instructions,
            live,
            filtered_out_count,
            collision_skipped_count,
        })
    }

    fn build(self) -> Option<McpBundle> {
        if self.sessions.is_empty() {
            return None;
        }
        Some(McpBundle {
            tools: self.tools,
            sessions: tokio::sync::Mutex::new(self.sessions),
            maintenance: tokio::sync::Mutex::new(self.maintenance),
        })
    }
}

#[cfg(test)]
static MCP_RUNTIME_CONSTRUCTIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
