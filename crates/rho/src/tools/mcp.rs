use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    net::IpAddr,
    path::{Path, PathBuf},
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
    ProcessExecution, ProcessInvocation, ProcessOutputLimits, Workspace,
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
pub(crate) mod report;

pub(crate) use report::{
    McpLoadMode, McpServerReport, McpServerStatus, McpSessionReport, McpToolReport,
    McpTransportSummary,
};

type McpSession = RunningService<RoleClient, ClientInfo>;

// The local end-to-end fixture initializes in about 40 ms. Two minutes leaves
// a 3,000x margin for cold package runners while still bounding broken servers.
const MCP_SERVER_STARTUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

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

/// Whether this session should connect MCP servers or only inventory config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpSessionPlan {
    /// Native runtime with tools: connect enabled servers.
    Connect,
    /// Emit config inventory without starting transports.
    Inventory(McpLoadMode),
}

pub(crate) struct McpConnectOutcome {
    pub(crate) report: McpSessionReport,
    pub(crate) bundle: Option<McpBundle>,
}

impl McpConnectOutcome {
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            report: McpSessionReport::default(),
            bundle: None,
        }
    }

    /// Run the session plan against config: connect or inventory-only.
    pub(crate) async fn run(plan: McpSessionPlan, config: &McpConfig) -> Self {
        match plan {
            McpSessionPlan::Connect => McpBundle::connect(config).await,
            McpSessionPlan::Inventory(mode) => Self {
                report: McpSessionReport::from_config_unloaded(config, mode),
                bundle: None,
            },
        }
    }
}

pub(crate) struct McpBundle {
    tools: Vec<Arc<dyn Tool>>,
    sessions: tokio::sync::Mutex<Vec<McpSession>>,
}

impl McpBundle {
    /// Connect enabled servers in parallel and discover their tools. Always
    /// returns a structured inventory. The no-enabled-server path exits before
    /// allocating a transport, client, task, or bundle.
    pub(crate) async fn connect(config: &McpConfig) -> McpConnectOutcome {
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
                async move {
                    let transport = McpTransportSummary::from_server(&server);
                    let result = tokio::time::timeout(
                        MCP_SERVER_STARTUP_BUDGET,
                        connect_server(&identity, &server),
                    )
                    .await;
                    (identity, server, transport, result)
                }
            });
        // BTreeMap iteration order is preserved by join_all input order.
        let connect_results = futures_util::future::join_all(connect_jobs).await;

        let mut sessions = Vec::new();
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        let mut registered_names = HashSet::new();
        for (identity, server, transport, result) in connect_results {
            let connected = match result {
                Ok(Ok(connected)) => connected,
                Ok(Err(error)) => {
                    tracing::warn!(server = %identity, error = %error, "MCP server failed to initialize");
                    servers.push(McpServerReport::failed(
                        identity,
                        transport,
                        error.to_string(),
                    ));
                    continue;
                }
                Err(_) => {
                    tracing::warn!(
                        server = %identity,
                        limit_seconds = MCP_SERVER_STARTUP_BUDGET.as_secs(),
                        "MCP server exceeded its startup budget"
                    );
                    servers.push(McpServerReport::timed_out(
                        identity,
                        transport,
                        MCP_SERVER_STARTUP_BUDGET.as_secs(),
                    ));
                    continue;
                }
            };
            let (session, discovered) = connected;
            let permission = PermissionTarget::from_server(&server);
            let mut exported = Vec::new();
            let mut filtered_out_count = 0usize;
            let mut collision_skipped_count = 0usize;
            for remote in discovered {
                let remote_name = remote.name.to_string();
                if !server.tools.includes(&remote_name) {
                    filtered_out_count += 1;
                    continue;
                }
                let name = namespaced_tool_name(&identity, &remote_name);
                if !registered_names.insert(name.clone()) {
                    tracing::warn!(server = %identity, tool = %remote_name, exported = %name, "MCP tool name collision; ignoring tool");
                    collision_skipped_count += 1;
                    continue;
                }
                let description = remote
                    .description
                    .as_deref()
                    .unwrap_or("No description supplied by the MCP server");
                let tool = McpTool {
                    spec: ToolSpec {
                        name: name.clone(),
                        description: format!("MCP server `{identity}`: {description}"),
                        input_schema: serde_json::Value::Object((*remote.input_schema).clone()),
                    },
                    remote_name: remote_name.clone(),
                    peer: session.peer().clone(),
                    permission: permission.clone(),
                };
                tools.push(Arc::new(tool));
                exported.push(McpToolReport {
                    remote_name,
                    exported_name: name,
                });
            }
            servers.push(McpServerReport::connected(
                identity,
                transport,
                exported,
                filtered_out_count,
                collision_skipped_count,
            ));
            sessions.push(session);
        }

        servers.sort_by(|left, right| left.identity.cmp(&right.identity));
        let bundle = if sessions.is_empty() {
            None
        } else {
            Some(Self {
                tools,
                sessions: tokio::sync::Mutex::new(sessions),
            })
        };
        McpConnectOutcome {
            report: McpSessionReport {
                mode: McpLoadMode::Native,
                servers,
            },
            bundle,
        }
    }

    /// Close live MCP sessions. Used by CLI inspect after printing inventory.
    pub(crate) async fn close(&self) {
        let mut sessions = self.sessions.lock().await;
        for session in sessions.iter_mut() {
            if let Err(error) = session.close().await {
                tracing::warn!(error = %error, "MCP session shutdown failed");
            }
        }
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

async fn connect_server(
    identity: &str,
    server: &McpServerConfig,
) -> anyhow::Result<(McpSession, Vec<rmcp::model::Tool>)> {
    prepare_server_filesystem(server)?;
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
            apply_stdio_environment(&mut command, env, env_from_env)?;
            let transport = TokioChildProcess::new(command)
                .with_context(|| format!("failed to spawn MCP server `{identity}`"))?;
            ClientInfo::default().serve(transport).await?
        }
        McpTransport::StreamableHttp {
            url,
            headers: literal_headers,
            headers_from_env,
        } => {
            parse_remote_url(url)?;
            validate_literal_headers(literal_headers)?;
            validate_environment_header_names(headers_from_env)?;
            let mut headers = HashMap::new();
            // Literal headers apply first; environment-derived headers
            // override them on a name collision.
            for (name, value) in literal_headers {
                headers.insert(
                    HeaderName::try_from(name)
                        .with_context(|| format!("invalid header `{name}`"))?,
                    HeaderValue::try_from(value)
                        .with_context(|| format!("invalid value for MCP header `{name}`"))?,
                );
            }
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
            // rmcp's reqwest transport disables redirects, so configured
            // headers never cross origins through a redirect. This satisfies
            // the Agent Plugins header-forwarding rule.
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

fn apply_stdio_environment(
    command: &mut tokio::process::Command,
    env: &BTreeMap<String, String>,
    env_from_env: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
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
    Ok(())
}

/// Child environment names the stdio transport actually installs.
fn stdio_process_environment(
    env: &BTreeMap<String, String>,
    env_from_env: &BTreeMap<String, String>,
) -> ProcessEnvironment {
    let mut variable_names: Vec<String> = SAFE_CHILD_ENVIRONMENT
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    variable_names.extend(env.keys().cloned());
    variable_names.extend(env_from_env.keys().cloned());
    variable_names.sort_unstable();
    variable_names.dedup();
    ProcessEnvironment::InheritListed { variable_names }
}

fn prepare_server_filesystem(server: &McpServerConfig) -> anyhow::Result<()> {
    let Some(policy) = &server.filesystem else {
        return Ok(());
    };
    let storage = Workspace::new(&policy.directory_root).with_context(|| {
        format!(
            "cannot resolve package storage root `{}`",
            policy.directory_root.display()
        )
    })?;
    let requested_directory = storage.root().join(&policy.directory_relative_to_root);
    let directory = storage
        .resolve_for_write(&requested_directory)
        .with_context(|| {
            format!(
                "package data directory `{}` escapes its storage root",
                requested_directory.display()
            )
        })?;
    std::fs::create_dir_all(directory.path()).with_context(|| {
        format!(
            "cannot create package data directory `{}`",
            directory.path().display()
        )
    })?;
    storage
        .resolve_for_read(directory.path())
        .with_context(|| {
            format!(
                "cannot revalidate package data directory `{}` after creation",
                directory.path().display()
            )
        })?;

    let (primary_root, granted_roots) = policy
        .allowed_roots
        .split_first()
        .context("package MCP filesystem policy has no allowed roots")?;
    let mut allowed = Workspace::new(primary_root).with_context(|| {
        format!(
            "cannot resolve allowed MCP root `{}`",
            primary_root.display()
        )
    })?;
    for root in granted_roots {
        allowed = allowed
            .with_granted_root(root)
            .with_context(|| format!("cannot resolve allowed MCP root `{}`", root.display()))?;
    }
    if let McpTransport::Stdio { command, cwd, .. } = &server.transport {
        let command_path = Path::new(command);
        if command_path.is_absolute() {
            allowed.resolve_for_read(command_path).with_context(|| {
                format!(
                    "MCP command `{}` escapes its permitted roots",
                    command_path.display()
                )
            })?;
        }
        if let Some(cwd) = cwd {
            allowed.resolve_for_read(cwd).with_context(|| {
                format!(
                    "MCP working directory `{}` escapes its permitted roots",
                    cwd.display()
                )
            })?;
        }
    }
    Ok(())
}

pub(super) fn validate_identity(identity: &str) -> anyhow::Result<()> {
    if identity.is_empty()
        || !identity
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("server identity must contain only ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

pub(crate) fn parse_remote_url(value: &str) -> anyhow::Result<url::Url> {
    let url = url::Url::parse(value).context("invalid Streamable HTTP URL")?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => bail!("remote MCP URL must have a host"),
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("remote MCP URL must use HTTPS unless its host is loopback");
    }
    Ok(url)
}

pub(crate) fn validate_literal_headers(headers: &BTreeMap<String, String>) -> anyhow::Result<()> {
    validate_header_names(headers.keys())?;
    for value in headers.values() {
        HeaderValue::try_from(value).context("invalid MCP header value")?;
    }
    Ok(())
}

pub(crate) fn validate_environment_header_names(
    headers: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    validate_header_names(headers.keys())
}

fn validate_header_names<'a>(names: impl IntoIterator<Item = &'a String>) -> anyhow::Result<()> {
    let mut parsed = HashSet::new();
    for name in names {
        let name = HeaderName::try_from(name).context("invalid MCP header name")?;
        if !parsed.insert(name) {
            bail!("MCP headers repeat a name under different casing");
        }
    }
    Ok(())
}
fn namespaced_tool_name(server: &str, tool: &str) -> String {
    fn component(value: &str) -> String {
        const ESCAPE_PREFIX: &str = "_rho_";
        let already_safe = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if already_safe && !value.starts_with(ESCAPE_PREFIX) {
            return value.to_string();
        }

        let mut encoded = String::with_capacity(ESCAPE_PREFIX.len() + value.len() * 2);
        encoded.push_str(ESCAPE_PREFIX);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in value.bytes() {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        encoded
    }
    format!("mcp__{}__{}", component(server), component(tool))
}

#[derive(Clone)]
enum PermissionTarget {
    Process {
        command: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        environment: ProcessEnvironment,
    },
    Network {
        url: String,
    },
}

impl PermissionTarget {
    fn from_server(server: &McpServerConfig) -> Self {
        match &server.transport {
            McpTransport::Stdio {
                command,
                args,
                cwd,
                env,
                env_from_env,
            } => Self::Process {
                command: command.clone(),
                args: args.clone(),
                cwd: cwd.clone(),
                environment: stdio_process_environment(env, env_from_env),
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
            Self::Process {
                command,
                args,
                cwd,
                environment,
            } => {
                let invocation = ProcessInvocation::executable_from_path(command, args.clone());
                let execution = ProcessExecution::new(
                    cwd.clone()
                        .or_else(|| workspace_root.map(std::path::Path::to_path_buf))
                        .unwrap_or_default(),
                    invocation,
                    environment.clone(),
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
