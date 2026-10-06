//! Cursor's answers to the generic ACP runtime (`cursor-agent acp`).
//!
//! Pure policy plus one I/O step: `prepare` writes the Rho-managed Cursor
//! config dir that fences tools (see `acp_config`). Everything Cursor-specific
//! about the wire (extension methods, diff quirks) is delegated to sibling
//! `acp_*` modules.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    AuthMethod, AuthMethodId, RequestPermissionRequest, SessionModeId, SessionUpdate,
};
use serde_json::Value;

use crate::{
    acp_runtime::{permission::PermissionDecision, AcpAgentPolicy, AcpSpawnPlan, ExtensionAnswer},
    agent::CursorTool,
    cli_runtime::{status_sink::RuntimeLabel, CliExecutable},
    permission::PermissionMode,
};

use super::{
    acp_config::{self, CursorConfigPaths, CursorFence},
    acp_extensions, acp_permissions, executable,
    models::CURSOR_LABEL,
    spawn,
};

/// Cursor reads its config dir from this variable before XDG/home defaults.
const CURSOR_CONFIG_DIR_ENV: &str = "CURSOR_CONFIG_DIR";

/// Bound Cursor run policy. Built once per run; `prepare` must run before
/// `spawn_plan` because the spawn env points at the dir `prepare` writes.
pub(crate) struct CursorAcpPolicy {
    model: Option<String>,
    permission_mode: PermissionMode,
    tools: Vec<CursorTool>,
    cwd: PathBuf,
    config_paths: CursorConfigPaths,
    /// Set by `prepare`; permission requests before it fail closed.
    prepared: Option<PreparedFence>,
}

struct PreparedFence {
    fence: CursorFence,
    /// Rho-managed `CURSOR_CONFIG_DIR`.
    config_dir: PathBuf,
}

impl CursorAcpPolicy {
    pub(crate) fn new(
        model: Option<String>,
        permission_mode: PermissionMode,
        tools: Vec<CursorTool>,
        cwd: PathBuf,
        config_paths: CursorConfigPaths,
    ) -> Self {
        Self {
            model,
            permission_mode,
            tools,
            cwd,
            config_paths,
            prepared: None,
        }
    }
}

impl AcpAgentPolicy for CursorAcpPolicy {
    fn label(&self) -> RuntimeLabel {
        CURSOR_LABEL
    }

    fn resolve_executable(&self) -> Result<CliExecutable, String> {
        executable::resolve().map_err(|error| error.to_string())
    }

    fn spawn_plan(&self, frozen: Option<Vec<String>>) -> Result<AcpSpawnPlan, String> {
        let config_dir = &self
            .prepared
            .as_ref()
            .ok_or("cursor: internal error: spawn before the managed config dir was prepared")?
            .config_dir;
        let mut plan = spawn::build_spawn_plan(&spawn::CursorSpawnRequest {
            model: self.model.clone(),
            cwd: self.cwd.clone(),
        });
        if let Some(arguments) = frozen {
            plan = spawn::apply_frozen_identity_args(plan, &arguments);
        }
        Ok(AcpSpawnPlan {
            argv: plan.args.iter().map(OsString::from).collect(),
            cwd: plan.cwd,
            env: vec![(
                OsString::from(CURSOR_CONFIG_DIR_ENV),
                config_dir.clone().into_os_string(),
            )],
        })
    }

    fn prepare(&mut self, run_dir: &Path) -> Result<Vec<String>, String> {
        // Binding refuses these modes first; checking again keeps a run that
        // skipped binding closed, and fails through the terminal artifact.
        let allowed = spawn::map_permission_mode(self.permission_mode, &self.tools)
            .map_err(|error| error.to_string())?;
        let fence = acp_config::fence(self.permission_mode, allowed.tools());
        let mut notices = Vec::new();
        if let Some(warning) = super::session::unknown_cursor_model_warning(self.model.as_deref()) {
            tracing::warn!("{warning}");
            notices.push(warning);
        }
        let managed = acp_config::write_run_config(&self.config_paths.user_dir(), run_dir, &fence)
            .map_err(|error| format!("cursor: could not prepare managed config: {error:#}"))?;
        notices.extend(managed.notices);
        self.prepared = Some(PreparedFence {
            fence,
            config_dir: managed.dir,
        });
        Ok(notices)
    }

    fn log_path(&self, output_file: &Path) -> PathBuf {
        spawn::log_path(output_file)
    }

    /// Never authenticate: `cursor_login` errors with API-key auth, and
    /// `session/new` already fails fast with `authRequired` when logged out.
    fn auth_method(&self, _advertised: &[AuthMethod]) -> Option<AuthMethodId> {
        None
    }

    fn session_mode(&self) -> Option<SessionModeId> {
        match self.permission_mode {
            PermissionMode::Plan => Some(SessionModeId::new("plan")),
            PermissionMode::Bypass
            | PermissionMode::Auto
            | PermissionMode::AllowEdits
            | PermissionMode::Supervised => None,
        }
    }

    fn decide_permission(&self, request: &RequestPermissionRequest) -> PermissionDecision {
        match &self.prepared {
            Some(prepared) => {
                acp_permissions::decide(&prepared.fence, self.permission_mode, request)
            }
            None => PermissionDecision::Reject,
        }
    }

    fn answer_extension(&self, method: &str, params: &Value) -> Option<ExtensionAnswer> {
        acp_extensions::answer(self.permission_mode, method, params).map(|reply| ExtensionAnswer {
            reply,
            events: Vec::new(),
        })
    }

    fn normalize_update(&self, update: SessionUpdate) -> SessionUpdate {
        acp_extensions::normalize_update(update)
    }
}

#[cfg(test)]
#[path = "acp_policy_tests.rs"]
mod tests;
