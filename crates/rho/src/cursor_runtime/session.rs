//! Execute a `runtime: cursor` delegated run via `cursor-agent acp`.

use std::path::PathBuf;

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

use crate::cli_runtime::{parent_messages::ParentMessageInbox, CliSessionOverrides};

use crate::{
    acp_runtime::{self, AcpSessionRequest},
    agent::{CursorTool, PromptPolicy},
    permission::PermissionMode,
    run_artifacts::RunArtifactIdentity,
    subagent::RunStatus,
};

use super::{acp_config::CursorConfigPaths, acp_policy::CursorAcpPolicy};

/// Inputs for one Cursor Agent subagent run, including bound runtime values.
///
/// `AgentExecutor` builds this directly after typed binding; tests build the
/// same shape and fill [`Self::overrides`].
pub(crate) struct CursorSessionRequest {
    /// Agent system prompt policy. Extend is prepended to the first prompt;
    /// Replace is rejected (ACP has no system-role override).
    pub(crate) system_prompt: PromptPolicy,
    /// Bound snapshot stamped onto `result.json`. Spawn reads model from here
    /// so it cannot drift from the Starting identity.
    pub(crate) identity: RunArtifactIdentity,
    pub(crate) tools: Vec<CursorTool>,
    pub(crate) prompt: String,
    pub(crate) output_file: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// When set, the launcher already force-replaced `result.json` with this
    /// Starting status. The sink continues from it instead of rewriting.
    pub(crate) started_status: Option<RunStatus>,
    /// Parent messages become the next `session/prompt` after a turn ends.
    pub(crate) parent_messages: Option<ParentMessageInbox>,
    /// Where the user's Cursor config lives; the managed copy derives from it.
    pub(crate) config_paths: CursorConfigPaths,
    pub(crate) overrides: CliSessionOverrides,
}

/// Run one Cursor Agent session to completion, writing the subagent contract.
pub(crate) async fn run_session(request: CursorSessionRequest) -> anyhow::Result<()> {
    let CursorSessionRequest {
        system_prompt,
        identity,
        tools,
        prompt,
        output_file,
        cwd,
        permission_mode,
        cancellation,
        status_tx,
        started_status,
        parent_messages,
        config_paths,
        overrides,
    } = request;
    let policy = CursorAcpPolicy::new(
        identity.model.clone(),
        permission_mode,
        tools,
        cwd.clone(),
        config_paths,
    );
    let request = AcpSessionRequest {
        identity,
        prompt,
        system_prompt,
        output_file,
        cwd,
        cancellation,
        status_tx,
        started_status,
        parent_messages,
        overrides,
    };
    acp_runtime::run_session(request, policy).await
}

/// Warn when a pinned Cursor model is missing from a non-empty cache.
pub(super) fn unknown_cursor_model_warning(model: Option<&str>) -> Option<String> {
    let model = model.filter(|value| !value.is_empty())?;
    let cached = super::models::cached();
    if cached.is_empty() {
        return None;
    }
    let lookup = model.split_once('[').map(|(id, _)| id).unwrap_or(model);
    if cached.iter().any(|row| row.id == lookup) {
        return None;
    }
    Some(format!("cursor model '{model}' is not in the cached list"))
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
