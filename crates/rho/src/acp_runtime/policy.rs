//! Agent-owned decisions consumed by the generic ACP driver.

use super::permission::PermissionDecision;
use crate::cli_runtime::{agent_event::AgentEvent, status_sink::RuntimeLabel, CliExecutable};
use agent_client_protocol::schema::v1::{
    AuthMethod, AuthMethodId, Meta, RequestPermissionRequest, SessionConfigId,
    SessionConfigValueId, SessionModeId, SessionUpdate,
};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// What one ACP agent needs from Rho, and how Rho answers agent requests.
/// Implementors supply synchronous policy; only executable resolution and
/// `prepare` may perform I/O. `prepare` must only write per-run configuration
/// under `run_dir`. Never mutate process environment or await another ACP
/// request while answering a request.
/// The driver owns process supervision, timeout budgets, rendering, and terminal
/// writes. Extended instructions are prompt context, not a replacement of the
/// agent's system prompt (ACP has no system-role override).
pub(crate) trait AcpAgentPolicy: Send + 'static {
    fn label(&self) -> RuntimeLabel;
    fn resolve_executable(&self) -> Result<CliExecutable, String>;
    /// Regenerate permission-sensitive flags; frozen argv overlays identity only.
    fn spawn_plan(&self, frozen: Option<Vec<String>>) -> Result<AcpSpawnPlan, String>;
    /// Per-run setup; returned notices are rendered before the agent is spawned.
    fn prepare(&mut self, run_dir: &Path) -> Result<Vec<String>, String>;
    fn log_path(&self, output_file: &Path) -> PathBuf;
    fn auth_method(&self, advertised: &[AuthMethod]) -> Option<AuthMethodId>;
    /// A required mode must be advertised and successfully set before prompting.
    fn session_mode(&self) -> Option<SessionModeId>;
    /// `_meta` sent with `session/new`, for vendor session parameters such as
    /// tool filters.
    fn session_meta(&self) -> Option<Meta> {
        None
    }
    /// Select options to set, in order, after `session/new` and the mode.
    /// Each must be advertised with the asked value, or the run fails before
    /// prompting.
    fn session_config(&self) -> Vec<SessionConfigChoice> {
        Vec::new()
    }
    fn decide_permission(&self, request: &RequestPermissionRequest) -> PermissionDecision;
    /// Answer any blocking extension request, including methods without a
    /// session id or underscore prefix. None replies method_not_found.
    fn answer_extension(&self, method: &str, params: &Value) -> Option<ExtensionAnswer>;
    fn normalize_update(&self, update: SessionUpdate) -> SessionUpdate {
        update
    }
}

/// Fully materialized launch; environment overrides apply to this child only.
pub(crate) struct AcpSpawnPlan {
    pub(crate) argv: Vec<OsString>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: Vec<(OsString, OsString)>,
}

/// One required `session/set_config_option` select value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionConfigChoice {
    pub(crate) id: SessionConfigId,
    pub(crate) value: SessionConfigValueId,
}

pub(crate) struct ExtensionAnswer {
    pub(crate) reply: Value,
    pub(crate) events: Vec<AgentEvent>,
}
