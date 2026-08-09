//! Establishing, maintaining, and closing one MCP server session.
//!
//! Everything here is per-server. Startup is bounded so a broken server cannot
//! stall a session; after the handshake succeeds a maintenance task owns the
//! long-lived server-to-client traffic (tool-list changes, keepalive) so the
//! tool call path stays a plain request/response.

// `logging/setLevel` carries a SEP-2577 deprecation marker in rmcp while every
// shipping server still uses it.
#![expect(deprecated)]

use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::{bail, Context};
use http::{HeaderName, HeaderValue};
use rho_sdk::Workspace;
use rmcp::{
    model::{SetLevelRequestParams, Tool as RemoteTool},
    service::{PeerRequestOptions, RunningService},
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, which_command,
        StreamableHttpClientTransport, TokioChildProcess,
    },
    Peer, RoleClient, ServiceExt,
};

use super::{
    client::{McpClientHandler, McpEventReceiver, McpServerEvent},
    config::{McpServerConfig, McpTransport},
    definition::McpToolDefinition,
    progress::McpProgressRouter,
    report::McpLiveServerState,
    roots::McpRoots,
    tool::McpToolSlot,
    validate,
};

pub(super) type McpSession = RunningService<RoleClient, McpClientHandler>;

// The local end-to-end fixture initializes in about 40 ms. Two minutes leaves
// a 3,000x margin for cold package runners while still bounding broken servers.
pub(super) const MCP_SERVER_STARTUP_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(120);
// Graceful MCP session teardown should be quick; bound it so one hung server
// cannot stall process or CLI shutdown.
const MCP_SESSION_CLOSE_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);
// Remote sessions can be dropped by an idle proxy without any local signal, so
// they are pinged. A stdio child's death is observable directly and needs none.
const MCP_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
// One task owns all of a session's maintenance, so a server that accepts a
// request and never answers would stop every later tool-list change and
// reachability update. Bound each request well inside the keepalive interval so
// the next tick still gets its turn.
const MCP_MAINTENANCE_REQUEST_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) enum ConnectResult {
    Ready(Box<ConnectedServer>),
    Failed { error: anyhow::Error },
    TimedOut,
}

/// A server that finished its handshake and first discovery.
pub(super) struct ConnectedServer {
    pub(super) session: McpSession,
    pub(super) discovered: Vec<RemoteTool>,
    /// `initialize` guidance, passed to the model as server-authored context.
    pub(super) instructions: Option<String>,
    pub(super) progress: McpProgressRouter,
    pub(super) events: McpEventReceiver,
}

/// Establish and discover under one startup budget. After the session exists,
/// failures and timeouts always attempt a bounded close instead of relying on
/// Drop alone.
pub(super) async fn connect_server_bounded(
    identity: &str,
    server: &McpServerConfig,
    roots: &McpRoots,
) -> ConnectResult {
    let deadline = tokio::time::Instant::now() + MCP_SERVER_STARTUP_BUDGET;
    let progress = McpProgressRouter::new();
    let (event_sender, events) = tokio::sync::mpsc::unbounded_channel();
    let handler = McpClientHandler::new(identity, roots.clone(), progress.clone(), event_sender);
    let session =
        match tokio::time::timeout_at(deadline, establish_session(identity, server, handler)).await
        {
            Ok(Ok(session)) => session,
            Ok(Err(error)) => return ConnectResult::Failed { error },
            Err(_) => return ConnectResult::TimedOut,
        };

    let instructions = session
        .peer_info()
        .and_then(|info| info.instructions.clone())
        .filter(|instructions| !instructions.trim().is_empty());
    apply_log_level(identity, server, &session, deadline).await;

    match tokio::time::timeout_at(deadline, session.list_all_tools()).await {
        Ok(Ok(discovered)) => ConnectResult::Ready(Box::new(ConnectedServer {
            session,
            discovered,
            instructions,
            progress,
            events,
        })),
        Ok(Err(error)) => {
            close_session(session).await;
            ConnectResult::Failed {
                error: anyhow::anyhow!(error)
                    .context(format!("MCP server `{identity}` failed tools/list")),
            }
        }
        Err(_) => {
            close_session(session).await;
            ConnectResult::TimedOut
        }
    }
}

/// Ask the server to emit logs at the configured level. A server that does not
/// declare `logging` is left alone; asking anyway would fail the request and
/// tell the user nothing useful.
///
/// This runs inside the startup deadline. Logging is optional, so a server that
/// never answers it must not push startup past the budget the user is told
/// about; it spends the remaining budget and startup then times out as usual.
async fn apply_log_level(
    identity: &str,
    server: &McpServerConfig,
    session: &McpSession,
    deadline: tokio::time::Instant,
) {
    let Some(level) = server.log_level else {
        return;
    };
    let declares_logging = session
        .peer_info()
        .is_some_and(|info| info.capabilities.logging.is_some());
    if !declares_logging {
        tracing::debug!(
            server = %identity,
            "MCP server does not support logging; log_level was not applied"
        );
        return;
    }
    let set_level = session
        .peer()
        .set_level(SetLevelRequestParams::new(level.into()));
    match tokio::time::timeout_at(deadline, set_level).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(server = %identity, error = %error, "MCP logging/setLevel failed");
        }
        Err(_) => tracing::warn!(
            server = %identity,
            limit_seconds = MCP_SERVER_STARTUP_BUDGET.as_secs(),
            "MCP logging/setLevel exhausted the server startup budget"
        ),
    }
}

async fn establish_session(
    identity: &str,
    server: &McpServerConfig,
    handler: McpClientHandler,
) -> anyhow::Result<McpSession> {
    prepare_server_filesystem(server)?;
    match &server.transport {
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
            // Start from the shared sanitized base. Servers opt into all other
            // inherited variables through `env_from_env`.
            apply_stdio_environment(&mut command, env, env_from_env)?;
            let transport = TokioChildProcess::new(command)
                .with_context(|| format!("failed to spawn MCP server `{identity}`"))?;
            Ok(handler.serve(transport).await?)
        }
        McpTransport::StreamableHttp {
            url,
            headers: literal_headers,
            headers_from_env,
        } => {
            validate::parse_remote_url(url)?;
            let headers = resolve_headers(literal_headers, headers_from_env)?;
            // rmcp's reqwest transport disables redirects, so configured
            // headers never cross origins through a redirect. This satisfies
            // the Agent Plugins header-forwarding rule.
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url.clone()).custom_headers(headers),
            );
            Ok(handler.serve(transport).await?)
        }
    }
}

fn resolve_headers(
    literal_headers: &BTreeMap<String, String>,
    headers_from_env: &BTreeMap<String, String>,
) -> anyhow::Result<std::collections::HashMap<HeaderName, HeaderValue>> {
    validate::validate_literal_headers(literal_headers)?;
    validate::validate_environment_header_names(headers_from_env)?;
    let mut headers = std::collections::HashMap::new();
    // Literal headers apply first; environment-derived headers override them
    // on a name collision.
    for (name, value) in literal_headers {
        headers.insert(header_name(name)?, header_value(name, value)?);
    }
    for (name, variable) in headers_from_env {
        let value = std::env::var(variable).with_context(|| {
            format!("environment variable `{variable}` for MCP header `{name}` is not set")
        })?;
        headers.insert(header_name(name)?, header_value(name, &value)?);
    }
    Ok(headers)
}

fn header_name(name: &str) -> anyhow::Result<HeaderName> {
    HeaderName::try_from(name).with_context(|| format!("invalid header `{name}`"))
}

fn header_value(name: &str, value: &str) -> anyhow::Result<HeaderValue> {
    HeaderValue::try_from(value).with_context(|| format!("invalid value for MCP header `{name}`"))
}

pub(super) async fn close_session(mut session: McpSession) {
    match tokio::time::timeout(MCP_SESSION_CLOSE_BUDGET, session.close()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "MCP session shutdown failed");
        }
        Err(_) => {
            tracing::warn!(
                limit_seconds = MCP_SESSION_CLOSE_BUDGET.as_secs(),
                "MCP session shutdown exceeded its close budget"
            );
        }
    }
}

/// Everything the per-session maintenance task needs to keep one server's
/// exported tools current.
pub(super) struct SessionMaintenance {
    pub(super) identity: String,
    pub(super) peer: Peer<RoleClient>,
    pub(super) server: McpServerConfig,
    /// Remote tool name to the slot backing its exported native tool.
    pub(super) slots: BTreeMap<String, Arc<McpToolSlot>>,
    pub(super) live: McpLiveServerState,
    pub(super) events: McpEventReceiver,
}

/// Own the long-lived server-to-client work for one session.
///
/// The task ends when the handler drops, which happens when the session closes,
/// so shutdown needs no extra signal.
pub(super) async fn maintain_session(mut maintenance: SessionMaintenance) {
    let keepalive = matches!(
        maintenance.server.transport,
        McpTransport::StreamableHttp { .. }
    );
    let mut ticker = tokio::time::interval(MCP_KEEPALIVE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick completes immediately; a ping right after the handshake
    // says nothing useful.
    ticker.tick().await;
    loop {
        tokio::select! {
            event = maintenance.events.recv() => match event {
                Some(McpServerEvent::ToolsChanged) => refresh_tools(&maintenance).await,
                None => break,
            },
            _ = ticker.tick(), if keepalive => {
                if let Err(error) = ping(&maintenance.peer).await {
                    tracing::warn!(
                        server = %maintenance.identity,
                        error = %error,
                        "MCP keepalive ping failed"
                    );
                    maintenance.live.mark_unreachable(error.to_string());
                } else {
                    maintenance.live.mark_reachable();
                }
            }
        }
    }
}

/// Send an MCP `ping`. rmcp exposes no typed helper for it on the client peer,
/// so the request goes out through the generic path, under the request budget
/// rmcp applies to the handle: an unanswered ping fails and tells the server the
/// request was cancelled rather than holding the maintenance task.
async fn ping(peer: &Peer<RoleClient>) -> Result<(), rmcp::service::ServiceError> {
    let mut options = PeerRequestOptions::no_options();
    options.timeout = Some(MCP_MAINTENANCE_REQUEST_BUDGET);
    peer.send_cancellable_request(
        rmcp::model::ClientRequest::PingRequest(rmcp::model::PingRequest::default()),
        options,
    )
    .await?
    .await_response()
    .await
    .map(|_| ())
}

/// Re-run discovery and reconcile it against the tools exported at startup.
///
/// Revised descriptions and schemas reach the model on the next turn, because
/// Rho reads each tool's spec when it builds a run. A tool the server withdrew
/// starts failing with a clear reason. A tool the server added cannot join the
/// registry mid-session, so it is recorded for `/mcp` to report instead of
/// being silently dropped.
///
/// rmcp's paginated helper takes no request options, so the whole refresh runs
/// under one budget instead of rmcp's per-request one. That bounds a server that
/// answers pages slowly as well as one that never answers at all.
async fn refresh_tools(maintenance: &SessionMaintenance) {
    let discovered = match tokio::time::timeout(
        MCP_MAINTENANCE_REQUEST_BUDGET,
        maintenance.peer.list_all_tools(),
    )
    .await
    {
        Ok(Ok(discovered)) => discovered,
        Ok(Err(error)) => {
            tracing::warn!(
                server = %maintenance.identity,
                error = %error,
                "MCP tools/list refresh failed"
            );
            return;
        }
        Err(_) => {
            tracing::warn!(
                server = %maintenance.identity,
                limit_seconds = MCP_MAINTENANCE_REQUEST_BUDGET.as_secs(),
                "MCP tools/list refresh exceeded its budget"
            );
            return;
        }
    };

    let mut present = std::collections::HashSet::new();
    let mut added = Vec::new();
    for remote in discovered {
        let remote_name = remote.name.to_string();
        if !maintenance.server.tools.includes(&remote_name) {
            continue;
        }
        match maintenance.slots.get(&remote_name) {
            Some(slot) => {
                present.insert(remote_name.clone());
                if slot.refresh(McpToolDefinition::from_remote(
                    &maintenance.identity,
                    &remote_name,
                    &remote,
                )) {
                    tracing::info!(
                        server = %maintenance.identity,
                        tool = %remote_name,
                        "MCP tool definition updated"
                    );
                }
            }
            None => added.push(remote_name),
        }
    }

    let removed = maintenance
        .slots
        .iter()
        .filter(|(name, _)| !present.contains(name.as_str()))
        .map(|(name, slot)| {
            slot.withdraw();
            name.clone()
        })
        .collect::<Vec<_>>();
    maintenance.live.record_tool_changes(added, removed);
}

fn apply_stdio_environment(
    command: &mut tokio::process::Command,
    env: &BTreeMap<String, String>,
    env_from_env: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    crate::child_env::apply_base(command);
    command.envs(env);
    for (name, variable) in env_from_env {
        let value = std::env::var(variable).with_context(|| {
            format!("environment variable `{variable}` for MCP child variable `{name}` is not set")
        })?;
        command.env(name, value);
    }
    Ok(())
}

pub(super) fn prepare_server_filesystem(server: &McpServerConfig) -> anyhow::Result<()> {
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
