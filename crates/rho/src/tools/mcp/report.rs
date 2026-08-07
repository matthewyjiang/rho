//! Structured MCP session inventory for `/mcp`, `/doctor`, and `rho mcp`.

use std::path::Path;

use serde::Serialize;

use super::config::{McpConfig, McpServerConfig, McpTransport};

/// How the current process treated MCP configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpLoadMode {
    /// Native Rho tool runtime; enabled servers were connected when present.
    #[default]
    Native,
    /// Active agent has no native host tools (for example Claude CLI).
    UnsupportedAgent,
    /// Session started with tools disabled (`--no-tools`).
    ToolsDisabled,
}

/// Outcome for one configured or invalid MCP server entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpServerStatus {
    Connected,
    Disabled,
    InvalidConfig,
    Failed,
    TimedOut,
    /// Present in config but not loaded in this session/runtime.
    NotLoaded,
}

impl McpServerStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disabled => "disabled",
            Self::InvalidConfig => "invalid",
            Self::Failed => "failed",
            Self::TimedOut => "timeout",
            Self::NotLoaded => "not loaded",
        }
    }

    pub(crate) const fn is_healthy(self) -> bool {
        matches!(self, Self::Connected | Self::Disabled | Self::NotLoaded)
    }
}

/// Transport facts safe to show in UI and CLI output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum McpTransportSummary {
    Stdio { command: String, args: Vec<String> },
    StreamableHttp { url: String },
}

impl McpTransportSummary {
    pub(crate) fn from_server(server: &McpServerConfig) -> Self {
        match &server.transport {
            McpTransport::Stdio { command, args, .. } => Self::Stdio {
                command: command.clone(),
                args: args.clone(),
            },
            McpTransport::StreamableHttp { url, .. } => Self::StreamableHttp { url: url.clone() },
        }
    }

    pub(crate) fn kind_label(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::StreamableHttp { .. } => "streamable_http",
        }
    }

    pub(crate) fn endpoint_summary(&self) -> String {
        match self {
            Self::Stdio { command, args } => {
                if args.is_empty() {
                    command.clone()
                } else {
                    format!("{command} {}", args.join(" "))
                }
            }
            Self::StreamableHttp { url } => url.clone(),
        }
    }
}

/// One tool exported from a connected server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct McpToolReport {
    pub(crate) remote_name: String,
    pub(crate) exported_name: String,
}

/// Shared healthy/status/detail copy for doctor and `/mcp` chrome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpStatusPresentation {
    pub(crate) healthy: bool,
    pub(crate) status: String,
    pub(crate) detail: String,
}

/// Counts and mode facts shared by doctor and picker copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct McpSessionSummary {
    unconfigured: bool,
    mode: McpLoadMode,
    connected: usize,
    enabled: usize,
    tools: usize,
    problems: usize,
}

impl McpSessionSummary {
    fn healthy(self) -> bool {
        if self.unconfigured {
            return true;
        }
        match self.mode {
            McpLoadMode::Native => self.problems == 0,
            McpLoadMode::UnsupportedAgent => self.enabled == 0 && self.problems == 0,
            McpLoadMode::ToolsDisabled => true,
        }
    }
}

/// How to project configured servers when seeding inventory from config alone.
#[derive(Clone, Copy, Debug)]
enum ConfigInventorySeed {
    /// Invalid entries plus every configured server as not-loaded/disabled.
    Unloaded,
    /// Invalid entries plus disabled servers only (enabled filled after connect).
    ConnectBase,
}

/// Per-server inventory row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct McpServerReport {
    pub(crate) identity: String,
    pub(crate) enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) transport: Option<McpTransportSummary>,
    pub(crate) status: McpServerStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) tools: Vec<McpToolReport>,
    pub(crate) filtered_out_count: usize,
    pub(crate) collision_skipped_count: usize,
}

impl McpServerReport {
    fn base(
        identity: impl Into<String>,
        enabled: bool,
        transport: Option<McpTransportSummary>,
    ) -> Self {
        Self {
            identity: identity.into(),
            enabled,
            transport,
            status: McpServerStatus::Disabled,
            error: None,
            tools: Vec::new(),
            filtered_out_count: 0,
            collision_skipped_count: 0,
        }
    }

    pub(crate) fn disabled(identity: impl Into<String>, server: &McpServerConfig) -> Self {
        let mut report = Self::base(
            identity,
            false,
            Some(McpTransportSummary::from_server(server)),
        );
        report.status = McpServerStatus::Disabled;
        report
    }

    pub(crate) fn invalid(identity: impl Into<String>, error: impl Into<String>) -> Self {
        let mut report = Self::base(identity, false, None);
        report.status = McpServerStatus::InvalidConfig;
        report.error = Some(error.into());
        report
    }

    pub(crate) fn not_loaded(identity: impl Into<String>, server: &McpServerConfig) -> Self {
        let enabled = server.enabled;
        let mut report = Self::base(
            identity,
            enabled,
            Some(McpTransportSummary::from_server(server)),
        );
        report.status = if enabled {
            McpServerStatus::NotLoaded
        } else {
            McpServerStatus::Disabled
        };
        report
    }

    pub(crate) fn timed_out(
        identity: impl Into<String>,
        transport: McpTransportSummary,
        limit_seconds: u64,
    ) -> Self {
        let mut report = Self::base(identity, true, Some(transport));
        report.status = McpServerStatus::TimedOut;
        report.error = Some(format!("exceeded {limit_seconds}s startup budget"));
        report
    }

    pub(crate) fn failed(
        identity: impl Into<String>,
        transport: McpTransportSummary,
        error: impl Into<String>,
    ) -> Self {
        let mut report = Self::base(identity, true, Some(transport));
        report.status = McpServerStatus::Failed;
        report.error = Some(error.into());
        report
    }

    pub(crate) fn connected(
        identity: impl Into<String>,
        transport: McpTransportSummary,
        tools: Vec<McpToolReport>,
        filtered_out_count: usize,
        collision_skipped_count: usize,
    ) -> Self {
        let mut report = Self::base(identity, true, Some(transport));
        report.status = McpServerStatus::Connected;
        report.tools = tools;
        report.filtered_out_count = filtered_out_count;
        report.collision_skipped_count = collision_skipped_count;
        report
    }

    pub(crate) fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Multi-line detail body for `/mcp` server rows.
    pub(crate) fn detail_text(&self) -> String {
        let mut lines = Vec::new();
        match self.transport.as_ref() {
            Some(transport) => {
                lines.push(format!(
                    "{} · {}",
                    transport.kind_label(),
                    transport.endpoint_summary()
                ));
            }
            None => lines.push("transport unavailable".into()),
        }
        if let Some(error) = self.error.as_deref() {
            lines.push(format!("error: {error}"));
        }
        match self.status {
            McpServerStatus::Connected if self.tools.is_empty() => {
                lines.push("connected with no exported tools".into());
            }
            McpServerStatus::Connected => {
                let names = self
                    .tools
                    .iter()
                    .map(|tool| tool.exported_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("{} tool(s): {names}", self.tool_count()));
            }
            McpServerStatus::Disabled => lines.push("enabled = false".into()),
            McpServerStatus::NotLoaded => {
                lines.push("configured but not loaded in this session".into());
            }
            McpServerStatus::InvalidConfig
            | McpServerStatus::Failed
            | McpServerStatus::TimedOut => {}
        }
        if self.filtered_out_count > 0 {
            lines.push(format!(
                "filtered out {} tool(s) by allow/deny",
                self.filtered_out_count
            ));
        }
        if self.collision_skipped_count > 0 {
            lines.push(format!(
                "skipped {} colliding tool name(s)",
                self.collision_skipped_count
            ));
        }
        lines.join("\n")
    }
}

/// Full MCP inventory for one session or CLI inspect run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct McpSessionReport {
    pub(crate) mode: McpLoadMode,
    pub(crate) servers: Vec<McpServerReport>,
}

impl McpSessionReport {
    /// Build inventory from config without connecting (unsupported agent or no-tools).
    pub(crate) fn from_config_unloaded(config: &McpConfig, mode: McpLoadMode) -> Self {
        let mut servers = seed_servers_from_config(config, ConfigInventorySeed::Unloaded);
        sort_servers(&mut servers);
        Self { mode, servers }
    }

    /// Invalid + disabled rows only; caller appends live connect outcomes.
    pub(crate) fn connect_base(config: &McpConfig) -> Self {
        Self {
            mode: McpLoadMode::Native,
            servers: seed_servers_from_config(config, ConfigInventorySeed::ConnectBase),
        }
    }

    pub(crate) fn is_unconfigured(&self) -> bool {
        self.servers.is_empty()
    }

    pub(crate) fn find(&self, identity: &str) -> Option<&McpServerReport> {
        self.servers
            .iter()
            .find(|server| server.identity == identity)
    }

    pub(crate) fn connected_count(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| server.status == McpServerStatus::Connected)
            .count()
    }

    pub(crate) fn problem_count(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| {
                matches!(
                    server.status,
                    McpServerStatus::Failed
                        | McpServerStatus::TimedOut
                        | McpServerStatus::InvalidConfig
                )
            })
            .count()
    }

    pub(crate) fn enabled_count(&self) -> usize {
        self.servers.iter().filter(|server| server.enabled).count()
    }

    pub(crate) fn exported_tool_count(&self) -> usize {
        self.servers.iter().map(McpServerReport::tool_count).sum()
    }

    fn summary(&self) -> McpSessionSummary {
        McpSessionSummary {
            unconfigured: self.is_unconfigured(),
            mode: self.mode,
            connected: self.connected_count(),
            enabled: self.enabled_count(),
            tools: self.exported_tool_count(),
            problems: self.problem_count(),
        }
    }

    /// `/doctor` MCP row copy.
    pub(crate) fn doctor_presentation(&self) -> McpStatusPresentation {
        format_presentation(self.summary(), PresentationSurface::Doctor)
    }

    /// `/mcp` session status row copy.
    pub(crate) fn picker_session_presentation(&self, config_path: &Path) -> McpStatusPresentation {
        format_presentation(
            self.summary(),
            PresentationSurface::Picker {
                config_path: crate::paths::display(config_path),
            },
        )
    }
}

#[derive(Clone, Debug)]
enum PresentationSurface {
    Doctor,
    Picker { config_path: String },
}

fn format_presentation(
    summary: McpSessionSummary,
    surface: PresentationSurface,
) -> McpStatusPresentation {
    let healthy = summary.healthy();
    if summary.unconfigured {
        let detail = match &surface {
            PresentationSurface::Doctor => "No MCP servers under [mcp.servers].".into(),
            PresentationSurface::Picker { config_path } => {
                format!("No MCP servers in {config_path}. Add entries under [mcp.servers].")
            }
        };
        return McpStatusPresentation {
            healthy,
            status: "not configured".into(),
            detail,
        };
    }

    match summary.mode {
        McpLoadMode::Native => {
            let McpSessionSummary {
                connected,
                enabled,
                tools,
                problems,
                ..
            } = summary;
            match surface {
                PresentationSurface::Doctor if problems == 0 => McpStatusPresentation {
                    healthy,
                    status: if connected > 0 {
                        "connected".into()
                    } else {
                        "idle".into()
                    },
                    detail: format!(
                        "{connected} connected server{}, {tools} exported tool{}.",
                        plural_suffix(connected),
                        plural_suffix(tools),
                    ),
                },
                PresentationSurface::Doctor => McpStatusPresentation {
                    healthy,
                    status: "degraded".into(),
                    detail: format!(
                        "{problems} server problem{}, {connected} connected, {tools} tools. Run /mcp for details.",
                        plural_suffix(problems),
                    ),
                },
                PresentationSurface::Picker { config_path } => McpStatusPresentation {
                    healthy,
                    status: format!("{connected} connected"),
                    detail: format!(
                        "{enabled} enabled, {tools} exported tool{}, {problems} problem{}. Config: {config_path}.",
                        plural_suffix(tools),
                        plural_suffix(problems),
                    ),
                },
            }
        }
        McpLoadMode::UnsupportedAgent => {
            let detail = match surface {
                PresentationSurface::Doctor => {
                    "Native MCP loads only for Rho agents. The active agent does not host MCP tools."
                        .into()
                }
                PresentationSurface::Picker { config_path } => format!(
                    "Native MCP loads only for Rho agents. The active agent does not host MCP tools. Config: {config_path}."
                ),
            };
            McpStatusPresentation {
                healthy,
                status: "unsupported agent".into(),
                detail,
            }
        }
        McpLoadMode::ToolsDisabled => {
            let detail = match surface {
                PresentationSurface::Doctor => {
                    "This session started with tools disabled, so MCP was not connected.".into()
                }
                PresentationSurface::Picker { config_path } => format!(
                    "This session started with tools disabled, so MCP was not connected. Config: {config_path}."
                ),
            };
            McpStatusPresentation {
                healthy,
                status: "tools disabled".into(),
                detail,
            }
        }
    }
}

fn seed_servers_from_config(config: &McpConfig, seed: ConfigInventorySeed) -> Vec<McpServerReport> {
    let mut servers = Vec::with_capacity(config.servers.len() + config.invalid_servers.len());
    for invalid in &config.invalid_servers {
        servers.push(McpServerReport::invalid(
            invalid.identity.clone(),
            invalid.error.clone(),
        ));
    }
    for (identity, server) in &config.servers {
        match seed {
            ConfigInventorySeed::Unloaded => {
                servers.push(McpServerReport::not_loaded(identity.clone(), server));
            }
            ConfigInventorySeed::ConnectBase if !server.enabled => {
                servers.push(McpServerReport::disabled(identity.clone(), server));
            }
            ConfigInventorySeed::ConnectBase => {}
        }
    }
    servers
}

pub(super) fn sort_servers(servers: &mut [McpServerReport]) {
    servers.sort_by(|left, right| left.identity.cmp(&right.identity));
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_healthy_matches_native_problems() {
        let report = McpSessionReport {
            mode: McpLoadMode::Native,
            servers: vec![McpServerReport::failed(
                "broken",
                McpTransportSummary::Stdio {
                    command: "false".into(),
                    args: Vec::new(),
                },
                "nope",
            )],
        };
        let doctor = report.doctor_presentation();
        assert!(!doctor.healthy);
        assert_eq!(doctor.status, "degraded");
        let picker = report.picker_session_presentation(Path::new("/tmp/config.toml"));
        assert!(!picker.healthy);
        assert!(picker.detail.contains("/tmp/config.toml"));
    }
}
