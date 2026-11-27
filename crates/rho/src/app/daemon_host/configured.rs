//! `rho/mcp/list` and `rho/hooks/list`: what the daemon's configuration
//! sets up, read for a settings screen without loading a session, starting
//! an MCP server, or running a hook. Each request reads the files again.
//!
//! The asking client's environment grants project trust as it does a
//! session that client opens, so the listing matches what such a session
//! would load.

use std::path::{Path, PathBuf};

use rho_protocol::{
    ClientEnvironment, ErrorCode, HooksListParams, HooksListResponse, McpListParams,
    McpListResponse, RpcError,
};

use super::{
    sessions::{session_environment, Workspace},
    settings::ConfigStore,
};
use crate::{app::configured_mcp::ConfiguredMcp, workspace::ProjectTrusts};

/// Answers `rho/mcp/list`: the servers of `config.toml` as a new session
/// reads it, then those of the plugins a session in the params' workspace
/// loads, the daemon user's home standing in for an absent one.
pub(super) async fn mcp(
    params: McpListParams,
    settings: &ConfigStore,
) -> Result<McpListResponse, RpcError> {
    let (workspace, trust) = parse(params.workspace.as_deref(), &params.environment)?;
    let config = settings
        .config_to_open()
        .await
        .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))?;
    let cwd = workspace.or_else(crate::paths::home_dir);
    let servers = ConfiguredMcp::discover(&config.mcp, cwd.as_deref(), trust).listing();
    let config_path = crate::paths::without_verbatim_prefix(settings.agent_path().to_path_buf());
    Ok(McpListResponse::new(config_path, servers))
}

/// Answers `rho/hooks/list`: the user's hooks file and, given a workspace,
/// its project's.
pub(super) fn hooks(params: HooksListParams) -> Result<HooksListResponse, RpcError> {
    let (workspace, trust) = parse(params.workspace.as_deref(), &params.environment)?;
    Ok(HooksListResponse::new(crate::hooks::configured_for_cwd(
        workspace.as_deref(),
        trust,
    )))
}

/// The workspace a session would open in, parsed as `rho/session/new`
/// parses its `cwd`, and the project trust `environment` grants a session.
pub(super) fn parse(
    workspace: Option<&Path>,
    environment: &ClientEnvironment,
) -> Result<(Option<PathBuf>, ProjectTrusts), RpcError> {
    let workspace = workspace
        .map(|path| {
            Workspace::parse("workspace", path).map(|workspace| workspace.path().to_path_buf())
        })
        .transpose()?;
    let ambient = session_environment(environment)?;
    Ok((workspace, ambient.project_trusts()))
}
