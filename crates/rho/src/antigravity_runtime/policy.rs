//! Antigravity's answers to the generic ACP runtime (`agy_acp_server`).
//!
//! Uses the user's real Gemini home: no per-run config directory. The fence
//! is per session instead (see `fence`): the `enabledTools` allowlist, mode
//! `default` required on every run (so a configured `yolo` cannot auto-approve
//! tools), and one-shot permission answers. The model is a session config
//! option, not argv.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use agent_client_protocol::schema::v1::{
    AuthMethod, AuthMethodId, Meta, RequestPermissionRequest, SessionModeId,
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

use super::{
    executable,
    fence::{self, AllowedTools},
    home::AntigravityHome,
    ANTIGRAVITY_LABEL,
};

/// agy_acp_server's prompt-for-permission mode (1.3.0 also offers
/// `auto_edit` and `yolo`, which auto-approve tools).
const DEFAULT_MODE: &str = "default";
/// agy_acp_server's model select option id.
const MODEL_CONFIG_ID: &str = "model";

/// Bound Antigravity run policy. `prepare` must run before the session
/// starts: it gates the mode, checks sign-in, and fixes the fence.
pub(crate) struct AntigravityAcpPolicy {
    model: Option<String>,
    permission_mode: PermissionMode,
    tools: Vec<AntigravityTool>,
    cwd: PathBuf,
    home: AntigravityHome,
    /// `ANTIGRAVITY_HARNESS_PATH` pin (see `executable::harness_env`).
    spawn_env: Vec<(OsString, OsString)>,
    /// Set by `prepare`; permission requests before it fail closed.
    allowed: Option<AllowedTools>,
}

impl AntigravityAcpPolicy {
    pub(crate) fn new(
        model: Option<String>,
        permission_mode: PermissionMode,
        tools: Vec<AntigravityTool>,
        cwd: PathBuf,
        home: AntigravityHome,
        spawn_env: Vec<(OsString, OsString)>,
    ) -> Self {
        Self {
            model,
            permission_mode,
            tools,
            cwd,
            home,
            spawn_env,
            allowed: None,
        }
    }
}

impl AcpAgentPolicy for AntigravityAcpPolicy {
    fn label(&self) -> RuntimeLabel {
        ANTIGRAVITY_LABEL
    }

    fn resolve_executable(&self) -> Result<CliExecutable, String> {
        executable::resolve().map_err(|error| error.to_string())
    }

    /// Argv carries no identity (the model is a config option), so frozen
    /// workflow argv has nothing to overlay.
    fn spawn_plan(&self, _frozen: Option<Vec<String>>) -> Result<AcpSpawnPlan, String> {
        Ok(AcpSpawnPlan {
            argv: executable::server_args(),
            cwd: self.cwd.clone(),
            env: self.spawn_env.clone(),
        })
    }

    fn prepare(&mut self, _run_dir: &Path) -> Result<Vec<String>, String> {
        // Binding refuses these modes first; checking again keeps a run that
        // skipped binding closed, and fails through the terminal artifact.
        let allowed = fence::map_permission_mode(self.permission_mode, &self.tools)
            .map_err(|error| error.to_string())?;
        self.home.status().require_signed_in()?;
        self.allowed = Some(allowed);
        Ok(Vec::new())
    }

    fn log_path(&self, output_file: &Path) -> PathBuf {
        output_file.with_file_name(crate::subagent::LOG_FILE_NAME)
    }

    /// Never authenticate in a run: sign-in is interactive (browser) and
    /// belongs to `rho login antigravity`; `prepare` already requires it.
    fn auth_method(&self, _advertised: &[AuthMethod]) -> Option<AuthMethodId> {
        None
    }

    fn session_mode(&self) -> Option<SessionModeId> {
        Some(SessionModeId::new(DEFAULT_MODE))
    }

    fn session_meta(&self) -> Option<Meta> {
        self.allowed.as_ref().map(AllowedTools::session_meta)
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
        match &self.allowed {
            Some(allowed) => fence::decide(allowed, self.permission_mode, request),
            None => PermissionDecision::Reject,
        }
    }

    /// agy_acp_server 1.3.0 sends no extension requests to clients.
    fn answer_extension(&self, _method: &str, _params: &Value) -> Option<ExtensionAnswer> {
        None
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
