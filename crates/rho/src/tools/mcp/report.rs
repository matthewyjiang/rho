//! Structured MCP session inventory for CLI and TUI consumers.

use serde::Serialize;

use super::config::{McpConfig, McpServerConfig, McpTransport};

/// How the current process treated MCP configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpLoadMode {
    #[default]
    Native,
    UnsupportedAgent,
    ToolsDisabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpServerStatus {
    Connected,
    Disabled,
    InvalidConfig,
    Failed,
    TimedOut,
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
            Self::Stdio { command, args } if args.is_empty() => command.clone(),
            Self::Stdio { command, args } => format!("{command} {}", args.join(" ")),
            Self::StreamableHttp { url } => url.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct McpToolReport {
    pub(crate) remote_name: String,
    pub(crate) exported_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum McpServerState {
    Connected {
        tools: Vec<McpToolReport>,
        filtered_out_count: usize,
        collision_skipped_count: usize,
    },
    Disabled,
    InvalidConfig {
        error: String,
    },
    Failed {
        error: String,
    },
    TimedOut {
        error: String,
    },
    NotLoaded,
}

impl McpServerState {
    const fn status(&self) -> McpServerStatus {
        match self {
            Self::Connected { .. } => McpServerStatus::Connected,
            Self::Disabled => McpServerStatus::Disabled,
            Self::InvalidConfig { .. } => McpServerStatus::InvalidConfig,
            Self::Failed { .. } => McpServerStatus::Failed,
            Self::TimedOut { .. } => McpServerStatus::TimedOut,
            Self::NotLoaded => McpServerStatus::NotLoaded,
        }
    }

    const fn enabled(&self) -> bool {
        match self {
            Self::Connected { .. }
            | Self::Failed { .. }
            | Self::TimedOut { .. }
            | Self::NotLoaded => true,
            Self::Disabled | Self::InvalidConfig { .. } => false,
        }
    }

    fn error(&self) -> Option<&str> {
        match self {
            Self::InvalidConfig { error } | Self::Failed { error } | Self::TimedOut { error } => {
                Some(error)
            }
            Self::Connected { .. } | Self::Disabled | Self::NotLoaded => None,
        }
    }

    fn tools(&self) -> &[McpToolReport] {
        match self {
            Self::Connected { tools, .. } => tools,
            Self::Disabled
            | Self::InvalidConfig { .. }
            | Self::Failed { .. }
            | Self::TimedOut { .. }
            | Self::NotLoaded => &[],
        }
    }

    const fn filtered_out_count(&self) -> usize {
        match self {
            Self::Connected {
                filtered_out_count, ..
            } => *filtered_out_count,
            Self::Disabled
            | Self::InvalidConfig { .. }
            | Self::Failed { .. }
            | Self::TimedOut { .. }
            | Self::NotLoaded => 0,
        }
    }

    const fn collision_skipped_count(&self) -> usize {
        match self {
            Self::Connected {
                collision_skipped_count,
                ..
            } => *collision_skipped_count,
            Self::Disabled
            | Self::InvalidConfig { .. }
            | Self::Failed { .. }
            | Self::TimedOut { .. }
            | Self::NotLoaded => 0,
        }
    }
}

/// Per-server inventory row. The state enum prevents contradictory status,
/// error, enabled, and tool combinations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpServerReport {
    pub(crate) identity: String,
    pub(crate) transport: Option<McpTransportSummary>,
    state: McpServerState,
}

impl Serialize for McpServerReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            identity: &'a str,
            enabled: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            transport: Option<&'a McpTransportSummary>,
            status: McpServerStatus,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<&'a str>,
            tools: &'a [McpToolReport],
            filtered_out_count: usize,
            collision_skipped_count: usize,
        }

        Wire {
            identity: &self.identity,
            enabled: self.enabled(),
            transport: self.transport.as_ref(),
            status: self.status(),
            error: self.error(),
            tools: self.tools(),
            filtered_out_count: self.filtered_out_count(),
            collision_skipped_count: self.collision_skipped_count(),
        }
        .serialize(serializer)
    }
}

impl McpServerReport {
    pub(crate) fn disabled(identity: impl Into<String>, server: &McpServerConfig) -> Self {
        Self {
            identity: identity.into(),
            transport: Some(McpTransportSummary::from_server(server)),
            state: McpServerState::Disabled,
        }
    }

    pub(crate) fn invalid(identity: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            identity: identity.into(),
            transport: None,
            state: McpServerState::InvalidConfig {
                error: error.into(),
            },
        }
    }

    pub(crate) fn not_loaded(identity: impl Into<String>, server: &McpServerConfig) -> Self {
        Self {
            identity: identity.into(),
            transport: Some(McpTransportSummary::from_server(server)),
            state: if server.enabled {
                McpServerState::NotLoaded
            } else {
                McpServerState::Disabled
            },
        }
    }

    pub(crate) fn timed_out(
        identity: impl Into<String>,
        transport: McpTransportSummary,
        limit_seconds: u64,
    ) -> Self {
        Self {
            identity: identity.into(),
            transport: Some(transport),
            state: McpServerState::TimedOut {
                error: format!("exceeded {limit_seconds}s startup budget"),
            },
        }
    }

    pub(crate) fn failed(
        identity: impl Into<String>,
        transport: McpTransportSummary,
        error: impl Into<String>,
    ) -> Self {
        Self {
            identity: identity.into(),
            transport: Some(transport),
            state: McpServerState::Failed {
                error: error.into(),
            },
        }
    }

    pub(crate) fn connected(
        identity: impl Into<String>,
        transport: McpTransportSummary,
        tools: Vec<McpToolReport>,
        filtered_out_count: usize,
        collision_skipped_count: usize,
    ) -> Self {
        Self {
            identity: identity.into(),
            transport: Some(transport),
            state: McpServerState::Connected {
                tools,
                filtered_out_count,
                collision_skipped_count,
            },
        }
    }

    pub(crate) const fn status(&self) -> McpServerStatus {
        self.state.status()
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.state.enabled()
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.state.error()
    }

    pub(crate) fn tools(&self) -> &[McpToolReport] {
        self.state.tools()
    }

    pub(crate) fn tool_count(&self) -> usize {
        self.tools().len()
    }

    pub(crate) const fn filtered_out_count(&self) -> usize {
        self.state.filtered_out_count()
    }

    pub(crate) const fn collision_skipped_count(&self) -> usize {
        self.state.collision_skipped_count()
    }

    pub(crate) fn detail_text(&self) -> String {
        let mut lines = vec![self
            .transport
            .as_ref()
            .map(|transport| {
                format!(
                    "{} · {}",
                    transport.kind_label(),
                    transport.endpoint_summary()
                )
            })
            .unwrap_or_else(|| "transport unavailable".into())];
        if let Some(error) = self.error() {
            lines.push(format!("error: {error}"));
        }
        match self.status() {
            McpServerStatus::Connected if self.tools().is_empty() => {
                lines.push("connected with no exported tools".into());
            }
            McpServerStatus::Connected => {
                let names = self
                    .tools()
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
        if self.filtered_out_count() > 0 {
            lines.push(format!(
                "filtered out {} tool(s) by allow/deny",
                self.filtered_out_count()
            ));
        }
        if self.collision_skipped_count() > 0 {
            lines.push(format!(
                "skipped {} colliding tool name(s)",
                self.collision_skipped_count()
            ));
        }
        lines.join("\n")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct McpSessionSummary {
    pub(crate) mode: McpLoadMode,
    pub(crate) configured: bool,
    pub(crate) connected: usize,
    pub(crate) problems: usize,
    pub(crate) enabled: usize,
    pub(crate) exported_tools: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct McpSessionReport {
    pub(crate) mode: McpLoadMode,
    pub(crate) servers: Vec<McpServerReport>,
}

impl McpSessionReport {
    pub(crate) fn from_config_unloaded(config: &McpConfig, mode: McpLoadMode) -> Self {
        let mut servers = config
            .invalid_servers
            .iter()
            .map(|invalid| McpServerReport::invalid(&invalid.identity, &invalid.error))
            .chain(
                config
                    .servers
                    .iter()
                    .map(|(identity, server)| McpServerReport::not_loaded(identity, server)),
            )
            .collect::<Vec<_>>();
        servers.sort_by(|left, right| left.identity.cmp(&right.identity));
        Self { mode, servers }
    }

    pub(crate) fn find(&self, identity: &str) -> Option<&McpServerReport> {
        self.servers
            .iter()
            .find(|server| server.identity == identity)
    }

    pub(crate) fn summary(&self) -> McpSessionSummary {
        let mut summary = McpSessionSummary {
            mode: self.mode,
            configured: !self.servers.is_empty(),
            connected: 0,
            problems: 0,
            enabled: 0,
            exported_tools: 0,
        };
        for server in &self.servers {
            summary.enabled += usize::from(server.enabled());
            summary.exported_tools += server.tool_count();
            match server.status() {
                McpServerStatus::Connected => summary.connected += 1,
                McpServerStatus::InvalidConfig
                | McpServerStatus::Failed
                | McpServerStatus::TimedOut => summary.problems += 1,
                McpServerStatus::Disabled | McpServerStatus::NotLoaded => {}
            }
        }
        summary
    }
}
