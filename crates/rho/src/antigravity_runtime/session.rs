//! Execute a `runtime: antigravity` delegated run via `agy_acp_server`.

use std::path::PathBuf;

use tokio::sync::watch;

use rho_tools::cancellation::RunCancellation;

use crate::{
    acp_runtime::{self, AcpSessionRequest},
    agent::{AntigravityTool, PromptPolicy},
    cli_runtime::{parent_messages::ParentMessageInbox, CliSessionOverrides},
    permission::PermissionMode,
    run_artifacts::RunArtifactIdentity,
    subagent::RunStatus,
};

use super::{executable, home::AntigravityHome, policy::AntigravityAcpPolicy};

/// Inputs for one Antigravity subagent run, including bound runtime values.
pub(crate) struct AntigravitySessionRequest {
    /// Extend is prepended to the first prompt; Replace is rejected (ACP has
    /// no system-role override).
    pub(crate) system_prompt: PromptPolicy,
    /// Bound snapshot stamped onto `result.json`; the model comes from here.
    pub(crate) identity: RunArtifactIdentity,
    pub(crate) tools: Vec<AntigravityTool>,
    pub(crate) prompt: String,
    pub(crate) output_file: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) cancellation: RunCancellation,
    pub(crate) status_tx: Option<watch::Sender<RunStatus>>,
    /// Starting status the launcher already wrote to `result.json`.
    pub(crate) started_status: Option<RunStatus>,
    /// Parent messages become the next `session/prompt` after a turn ends.
    pub(crate) parent_messages: Option<ParentMessageInbox>,
    pub(crate) home: AntigravityHome,
    pub(crate) overrides: CliSessionOverrides,
}

/// Run one Antigravity session to completion, writing the subagent contract.
pub(crate) async fn run_session(request: AntigravitySessionRequest) -> anyhow::Result<()> {
    let AntigravitySessionRequest {
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
        home,
        overrides,
    } = request;
    let policy = AntigravityAcpPolicy::new(
        identity.model.clone(),
        permission_mode,
        tools,
        cwd.clone(),
        home,
        executable::harness_env_for_launch(overrides.executable.as_ref()),
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
