use std::{
    collections::{HashMap, HashSet},
    future::Future,
    net::IpAddr,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
};

use anyhow::{bail, Context};
use http::{HeaderName, HeaderValue};
use rho_sdk::{
    model::ToolSpec,
    tool::{
        OperationKind, PreparedToolInvocation, Tool, ToolError, ToolErrorKind, ToolInvocation,
        ToolMetadata, ToolOutput, ToolPreparationContext, ToolPrepareFuture, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget, ProcessEnvironment,
    ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};
use rmcp::{
    model::{CallToolRequestParams, ClientInfo},
    service::RunningService,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, which_command,
        StreamableHttpClientTransport, TokioChildProcess,
    },
    RoleClient, ServiceExt,
};

use super::sdk_registry::ToolBundle;
use config::{McpConfig, McpServerConfig, McpTransport};

pub(crate) mod config;

type McpSession = RunningService<RoleClient, ClientInfo>;

// The local end-to-end fixture initializes in about 40 ms. Two minutes leaves
// a 3,000x margin for cold package runners while still bounding broken servers.
const MCP_SERVER_STARTUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

pub(crate) struct McpBundle {
    tools: Vec<Arc<dyn Tool>>,
    sessions: tokio::sync::Mutex<Vec<McpSession>>,
}

impl McpBundle {
    /// Connect enabled servers and discover their tools. The disabled path exits
    /// before allocating a transport, client, task, or bundle.
    pub(crate) async fn connect(config: &McpConfig) -> Option<Self> {
        for invalid in &config.invalid_servers {
            tracing::warn!(
                server = %invalid.identity,
                error = %invalid.error,
                "ignoring invalid MCP server configuration"
            );
        }
        if !config.has_enabled_servers() {
            return None;
        }

        #[cfg(test)]
        MCP_RUNTIME_CONSTRUCTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut sessions = Vec::new();
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        let mut registered_names = HashSet::new();
        for (identity, server) in config.servers.iter().filter(|(_, server)| server.enabled) {
            let result = match tokio::time::timeout(
                MCP_SERVER_STARTUP_BUDGET,
                connect_server(identity, server),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        server = %identity,
                        limit_seconds = MCP_SERVER_STARTUP_BUDGET.as_secs(),
                        "MCP server exceeded its startup budget"
                    );
                    continue;
                }
            };
            let (session, discovered) = match result {
                Ok(connected) => connected,
                Err(error) => {
                    tracing::warn!(server = %identity, error = %error, "MCP server failed to initialize");
                    continue;
                }
            };
            let permission = PermissionTarget::from_server(server);
            for remote in discovered {
                let remote_name = remote.name.to_string();
                if !server.tools.includes(&remote_name) {
                    continue;
                }
                let name = namespaced_tool_name(identity, &remote_name);
                if !registered_names.insert(name.clone()) {
                    tracing::warn!(server = %identity, tool = %remote_name, exported = %name, "MCP tool name collision; ignoring tool");
                    continue;
                }
                let description = remote
                    .description
                    .as_deref()
                    .unwrap_or("No description supplied by the MCP server");
                let tool = McpTool {
                    spec: ToolSpec {
                        name,
                        description: format!("MCP server `{identity}`: {description}"),
                        input_schema: serde_json::Value::Object((*remote.input_schema).clone()),
                    },
                    remote_name,
                    peer: session.peer().clone(),
                    permission: permission.clone(),
                };
                tools.push(Arc::new(tool));
            }
            sessions.push(session);
        }

        Some(Self {
            tools,
            sessions: tokio::sync::Mutex::new(sessions),
        })
    }
}

impl ToolBundle for McpBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut sessions = self.sessions.lock().await;
            for session in sessions.iter_mut() {
                if let Err(error) = session.close().await {
                    tracing::warn!(error = %error, "MCP session shutdown failed");
                }
            }
        })
    }
}

const SAFE_CHILD_ENVIRONMENT: &[&str] = &[
    "APPDATA",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LOCALAPPDATA",
    "LOGNAME",
    "PATH",
    "PATHEXT",
    "SHELL",
    "SYSTEMROOT",
    "TEMP",
    "TERM",
    "TMP",
    "TMPDIR",
    "USER",
    "USERPROFILE",
    "WINDIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

async fn connect_server(
    identity: &str,
    server: &McpServerConfig,
) -> anyhow::Result<(McpSession, Vec<rmcp::model::Tool>)> {
    validate_identity(identity)?;
    let mut session = match &server.transport {
        McpTransport::Stdio {
            command,
            args,
            cwd,
            env,
            env_from_env,
        } => {
            if command.trim().is_empty() {
                bail!("stdio command must not be empty");
            }
            let mut command = which_command(command)
                .with_context(|| format!("MCP executable `{command}` was not found"))?;
            command.args(args);
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            // Start from a small non-secret environment. Servers opt into all
            // other inherited variables through `env_from_env`.
            command.env_clear();
            for variable in SAFE_CHILD_ENVIRONMENT {
                if let Some(value) = std::env::var_os(variable) {
                    command.env(variable, value);
                }
            }
            command.envs(env);
            for (name, variable) in env_from_env {
                let value = std::env::var(variable).with_context(|| {
                    format!("environment variable `{variable}` for MCP child variable `{name}` is not set")
                })?;
                command.env(name, value);
            }
            let transport = TokioChildProcess::new(command)
                .with_context(|| format!("failed to spawn MCP server `{identity}`"))?;
            ClientInfo::default().serve(transport).await?
        }
        McpTransport::StreamableHttp {
            url,
            headers_from_env,
        } => {
            validate_remote_url(url)?;
            let mut headers = HashMap::new();
            for (name, variable) in headers_from_env {
                let value = std::env::var(variable).with_context(|| {
                    format!("environment variable `{variable}` for MCP header `{name}` is not set")
                })?;
                headers.insert(
                    HeaderName::try_from(name)
                        .with_context(|| format!("invalid header `{name}`"))?,
                    HeaderValue::try_from(value)
                        .with_context(|| format!("invalid value for MCP header `{name}`"))?,
                );
            }
            // rmcp's reqwest transport disables redirects. This prevents custom
            // authorization headers from crossing origins.
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url.clone()).custom_headers(headers),
            );
            ClientInfo::default().serve(transport).await?
        }
    };
    let tools = match session.list_all_tools().await {
        Ok(tools) => tools,
        Err(error) => {
            let _ = session.close().await;
            return Err(error)
                .with_context(|| format!("MCP server `{identity}` failed tools/list"));
        }
    };
    Ok((session, tools))
}

fn validate_identity(identity: &str) -> anyhow::Result<()> {
    if identity.is_empty()
        || !identity
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("server identity must contain only ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

fn validate_remote_url(value: &str) -> anyhow::Result<()> {
    let url = url::Url::parse(value).context("invalid Streamable HTTP URL")?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("remote MCP URL must use HTTPS unless its host is loopback");
    }
    Ok(())
}

fn namespaced_tool_name(server: &str, tool: &str) -> String {
    fn component(value: &str) -> String {
        value
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect()
    }
    format!("mcp__{}__{}", component(server), component(tool))
}

#[derive(Clone)]
enum PermissionTarget {
    Process {
        command: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Network {
        url: String,
    },
}

impl PermissionTarget {
    fn from_server(server: &McpServerConfig) -> Self {
        match &server.transport {
            McpTransport::Stdio {
                command, args, cwd, ..
            } => Self::Process {
                command: command.clone(),
                args: args.clone(),
                cwd: cwd.clone(),
            },
            McpTransport::StreamableHttp { url, .. } => Self::Network { url: url.clone() },
        }
    }

    fn kind(&self) -> CapabilityKind {
        match self {
            Self::Process { .. } => CapabilityKind::Process,
            Self::Network { .. } => CapabilityKind::Network,
        }
    }

    fn request(
        &self,
        tool_name: &str,
        workspace_root: Option<&std::path::Path>,
    ) -> CapabilityRequest {
        let source = CapabilitySource::built_in_tool(tool_name);
        match self {
            Self::Process { command, args, cwd } => {
                let invocation = ProcessInvocation::executable_from_path(command, args.clone());
                let execution = ProcessExecution::new(
                    cwd.clone()
                        .or_else(|| workspace_root.map(std::path::Path::to_path_buf))
                        .unwrap_or_default(),
                    invocation,
                    ProcessEnvironment::InheritAll,
                    // This object describes an already-running configured MCP
                    // server to the approval layer; it does not impose a runtime budget.
                    ProcessOutputLimits::new(usize::MAX, None),
                );
                CapabilityRequest::process(execution, source)
            }
            Self::Network { url } => {
                CapabilityRequest::network(NetworkTarget::Url(url.clone()), source)
            }
        }
    }

    fn metadata(&self) -> ToolMetadata {
        match self {
            Self::Process { command, args, .. } => ToolMetadata::new()
                .operation(OperationKind::Execute)
                .command_summary(format!("{command} ({} arguments)", args.len())),
            Self::Network { url } => ToolMetadata::new()
                .operation(OperationKind::Network)
                .url(url.clone()),
        }
    }
}

async fn call_remote_tool(
    peer: &rmcp::Peer<RoleClient>,
    remote_name: String,
    arguments: serde_json::Map<String, serde_json::Value>,
    cancellation: &rho_sdk::CancellationToken,
) -> Result<String, ToolError> {
    let params = CallToolRequestParams::new(remote_name).with_arguments(arguments);
    let result = tokio::select! {
        result = peer.call_tool(params) => result,
        () = cancellation.cancelled() => return Err(ToolError::cancelled()),
    }
    .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
    let content = serde_json::to_string(&result)
        .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
    if result.is_error.unwrap_or(false) {
        return Err(ToolError::new(ToolErrorKind::Execution, content));
    }
    Ok(content)
}

struct McpTool {
    spec: ToolSpec,
    remote_name: String,
    peer: rmcp::Peer<RoleClient>,
    permission: PermissionTarget,
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([self.permission.kind()])
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let arguments = invocation.into_arguments();
        let workspace_root = context.workspace_root().map(std::path::Path::to_path_buf);
        Box::pin(async move {
            let Some(arguments) = arguments.as_object().cloned() else {
                return Err(ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "MCP tool arguments must be a JSON object",
                ));
            };
            let capability = self
                .permission
                .request(&self.spec.name, workspace_root.as_deref());
            let metadata = self.permission.metadata();
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [capability],
                metadata.clone(),
                move |context| {
                    Box::pin(async move {
                        let content = call_remote_tool(
                            &self.peer,
                            self.remote_name.clone(),
                            arguments,
                            context.cancellation(),
                        )
                        .await?;
                        Ok(ToolOutput::text(content).metadata(metadata))
                    })
                },
            ))
        })
    }
}

#[cfg(test)]
static MCP_RUNTIME_CONSTRUCTIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
