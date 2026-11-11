//! Antigravity's answers to the generic ACP runtime (`agy acp`).
//!
//! The fence is per run (see `fence`): the run's agy agent with the kept
//! tools, the agy session mode, and one-shot permission answers. The model is
//! a session config option, not argv.

use std::path::{Path, PathBuf};

use agent_client_protocol::v1::schema::{
    AuthMethod, AuthMethodId, RequestPermissionRequest, SessionModeId,
};
use serde_json::Value;

use crate::{
    acp_runtime::{
        permission::PermissionDecision, AcpAgentPolicy, AcpSpawnPlan, ExtensionAnswer,
        SessionConfigChoice,
    },
    agent::AntigravityTool,
    cli_runtime::{status_sink::RuntimeLabel, CliExecutable},
    permission::PermissionMode,
};

use super::{agent_file, executable, fence::AntigravityFence, ANTIGRAVITY_LABEL};

/// agy's model select option id.
const MODEL_CONFIG_ID: &str = "model";

/// Bound Antigravity run policy. `prepare` must run before the session
/// starts: it gates the mode, fixes the fence, and writes the run's agent.
pub(crate) struct AntigravityAcpPolicy {
    model: Option<String>,
    tools: Vec<AntigravityTool>,
    permission_mode: PermissionMode,
    cwd: PathBuf,
    /// Set by `prepare`; permission requests before it fail closed.
    prepared: Option<Prepared>,
}

struct Prepared {
    fence: AntigravityFence,
    agent: agent_file::RunAgent,
}

impl AntigravityAcpPolicy {
    pub(crate) fn new(
        model: Option<String>,
        tools: Vec<AntigravityTool>,
        permission_mode: PermissionMode,
        cwd: PathBuf,
    ) -> Self {
        Self {
            model,
            tools,
            permission_mode,
            cwd,
            prepared: None,
        }
    }
}

impl AcpAgentPolicy for AntigravityAcpPolicy {
    fn label(&self) -> RuntimeLabel {
        ANTIGRAVITY_LABEL
    }

    fn resolve_executable(
        &self,
        environment: &crate::session_environment::Ambient,
    ) -> Result<CliExecutable, String> {
        executable::resolve(environment).map_err(|error| error.to_string())
    }

    /// Argv carries no identity (the model is a config option), so frozen
    /// workflow argv has nothing to overlay.
    fn spawn_plan(&self, _frozen: Option<Vec<String>>) -> Result<AcpSpawnPlan, String> {
        let prepared = self
            .prepared
            .as_ref()
            .ok_or("antigravity: run was not prepared")?;
        Ok(AcpSpawnPlan {
            argv: executable::server_args(&prepared.agent),
            cwd: self.cwd.clone(),
            env: Vec::new(),
        })
    }

    fn prepare(&mut self, run_dir: &Path) -> Result<Vec<String>, String> {
        // Binding refuses these first; checking again keeps a run that skipped
        // binding closed, and fails through the terminal artifact.
        let fence = AntigravityFence::new(self.permission_mode, &self.tools)
            .map_err(|error| error.to_string())?;
        let agent = agent_file::write(run_dir, fence.tools()).map_err(|error| {
            format!("antigravity: could not write the run's agy agent: {error}")
        })?;
        self.prepared = Some(Prepared { fence, agent });
        Ok(Vec::new())
    }

    fn log_path(&self, output_file: &Path) -> PathBuf {
        output_file.with_file_name(crate::subagent::LOG_FILE_NAME)
    }

    /// Never authenticate in a run: sign-in is interactive and belongs to
    /// `/login antigravity`. A signed-out agy fails `session/new` with
    /// "Authentication required".
    fn auth_method(&self, _advertised: &[AuthMethod]) -> Option<AuthMethodId> {
        None
    }

    fn session_mode(&self) -> Option<SessionModeId> {
        self.prepared
            .as_ref()
            .map(|prepared| SessionModeId::new(prepared.fence.session_mode()))
    }

    fn session_config(&self) -> Vec<SessionConfigChoice> {
        self.model
            .iter()
            .map(|model| SessionConfigChoice {
                id: MODEL_CONFIG_ID.into(),
                value: model.clone().into(),
            })
            .collect()
    }

    fn decide_permission(&self, request: &RequestPermissionRequest) -> PermissionDecision {
        self.prepared
            .as_ref()
            .map_or(PermissionDecision::Reject, |prepared| {
                prepared.fence.decide(request)
            })
    }

    /// agy 1.5.0 sends no extension requests that Rho can answer.
    fn answer_extension(&self, _method: &str, _params: &Value) -> Option<ExtensionAnswer> {
        None
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
