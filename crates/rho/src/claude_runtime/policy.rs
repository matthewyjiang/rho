//! Claude Code's answers to the generic ACP runtime (`claude-agent-acp`).
//!
//! Everything Claude-specific rides `session/new` `_meta` (see the module
//! docs for the recorded protocol facts): the system prompt as
//! `_meta.systemPrompt`, and model, effort, the turn cap, tools, and the
//! settings fence as `_meta.claudeCode.options`, which the adapter passes to
//! the Claude Agent SDK. The Claude session mode and the permission answers
//! come from the [`ClaudeFence`].

use std::path::{Path, PathBuf};

use agent_client_protocol::v1::schema::{
    AuthMethod, AuthMethodId, Meta, RequestPermissionRequest, SessionModeId, SessionUpdate,
};
use serde_json::{json, Map, Value};

use crate::{
    acp_runtime::{permission::PermissionDecision, AcpAgentPolicy, AcpSpawnPlan, ExtensionAnswer},
    agent::PromptPolicy,
    cli_runtime::{
        status_sink::{RateLimitRecorder, RuntimeLabel},
        stream_effect::{RateLimitInfo, StatusPatch, StreamEffect},
        CliExecutable,
    },
    permission::PermissionMode,
};

use super::{
    executable,
    fence::{self, ClaudeFence, ClaudeFenceError},
    rate_limit::ClaudeRateLimitRecorder,
    session::CLAUDE_LABEL,
};

/// `usage_update._meta` key carrying the SDK's rate-limit event.
const RATE_LIMIT_META: &str = "_claude/rateLimit";
/// `usage_update._meta` key naming the model that produced the usage.
const MODEL_META: &str = "_claude/model";
/// The adapter's highest-priority session model source.
const MODEL_ENV: &str = "ANTHROPIC_MODEL";

/// What one Claude session is asked to be.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeSessionShape {
    pub(crate) system_prompt: PromptPolicy,
    /// Pass-through Claude model name or alias; `None` inherits Claude's.
    pub(crate) model: Option<String>,
    pub(crate) reasoning: Option<crate::agent::ReasoningLevel>,
    /// Declared `tools:` entries, including specifiers.
    pub(crate) tools: Vec<String>,
    pub(crate) max_turns: u64,
    /// Whether the conversation stays resumable in Claude's session store.
    pub(crate) persistence: SessionPersistence,
}

/// Whether a run leaves a resumable Claude session behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionPersistence {
    /// Delegated runs publish the session id so the user can resume it.
    Keep,
    /// Rho's own one-shot calls would otherwise fill the user's session list.
    Discard,
}

pub(crate) struct ClaudeAcpPolicy {
    shape: ClaudeSessionShape,
    /// A refused mode is kept so `prepare` fails the run through its terminal
    /// artifact before anything reaches the agent.
    fence: Result<ClaudeFence, ClaudeFenceError>,
    cwd: PathBuf,
    /// Rate-limit cache path. Tests inject a temp path so runs never touch
    /// the host cache.
    rate_limit_state_path: Option<PathBuf>,
}

impl ClaudeAcpPolicy {
    /// A delegated run under the bound Rho permission mode.
    pub(crate) fn delegated(
        shape: ClaudeSessionShape,
        permission_mode: PermissionMode,
        inherit_claude_config: bool,
        cwd: PathBuf,
        rate_limit_state_path: Option<PathBuf>,
    ) -> Self {
        Self {
            fence: ClaudeFence::new(permission_mode, &shape.tools, inherit_claude_config),
            shape,
            cwd,
            rate_limit_state_path,
        }
    }

    /// One of Rho's own no-tools questions, independent of the host mode.
    pub(crate) fn headless(shape: ClaudeSessionShape, cwd: PathBuf) -> Self {
        Self {
            shape,
            fence: Ok(ClaudeFence::headless()),
            cwd,
            rate_limit_state_path: None,
        }
    }

    /// `None` only for a refused mode, which `prepare` has already failed.
    fn fence(&self) -> Option<&ClaudeFence> {
        self.fence.as_ref().ok()
    }

    /// `_meta` for `session/new`; requires a fence.
    fn meta(&self, fence: &ClaudeFence) -> Meta {
        let shape = &self.shape;
        let mut options = Map::new();
        options.insert("tools".into(), json!(fence::tool_base_names(&shape.tools)));
        let allowed = fence::allowed_tool_entries(&shape.tools);
        if !allowed.is_empty() {
            options.insert("allowedTools".into(), json!(allowed));
        }
        options.insert(
            "disallowedTools".into(),
            json!(fence::DISALLOWED_NESTED_AGENT_TOOLS),
        );
        options.insert("settingSources".into(), json!(fence.setting_sources()));
        options.insert("strictMcpConfig".into(), json!(true));
        options.insert("maxTurns".into(), json!(shape.max_turns));
        // The adapter predicts a follow-up prompt per turn by default; Rho
        // never shows it.
        options.insert("promptSuggestions".into(), json!(false));
        if let Some(model) = &shape.model {
            options.insert("model".into(), json!(model));
        }
        if let Some(effort) = shape.reasoning.and_then(fence::claude_effort) {
            options.insert("effort".into(), json!(effort));
        }
        if shape.persistence == SessionPersistence::Discard {
            options.insert("persistSession".into(), json!(false));
        }
        let mut meta = Map::new();
        // A string replaces Claude's system prompt; `append` extends it.
        match &shape.system_prompt {
            PromptPolicy::Replace(text) => {
                meta.insert("systemPrompt".into(), json!(text));
            }
            PromptPolicy::Extend(text) if !text.trim().is_empty() => {
                meta.insert("systemPrompt".into(), json!({ "append": text }));
            }
            PromptPolicy::Extend(_) => {}
        }
        meta.insert("claudeCode".into(), json!({ "options": options }));
        meta
    }
}

impl AcpAgentPolicy for ClaudeAcpPolicy {
    fn label(&self) -> RuntimeLabel {
        CLAUDE_LABEL
    }

    fn resolve_executable(
        &self,
        environment: &crate::session_environment::Ambient,
    ) -> Result<CliExecutable, String> {
        executable::resolve(environment).map_err(|error| error.to_string())
    }

    /// The adapter takes no arguments: identity rides `session/new`. It runs
    /// its bundled Claude CLI, so a stray `CLAUDE_CODE_EXECUTABLE` is the
    /// user's own override.
    ///
    /// A model pin also goes in `ANTHROPIC_MODEL`: the adapter picks the
    /// session model from that variable, then the user's `settings.model`
    /// (read whatever `settingSources` says), and never from the `model`
    /// option, so without it a settings file would replace the pin.
    fn spawn_plan(&self, _frozen: Option<Vec<String>>) -> Result<AcpSpawnPlan, String> {
        let env = self
            .shape
            .model
            .iter()
            .map(|model| (MODEL_ENV.into(), model.into()))
            .collect();
        Ok(AcpSpawnPlan {
            argv: Vec::new(),
            cwd: self.cwd.clone(),
            env,
        })
    }

    /// Binding refuses these modes first; failing here keeps a run that
    /// skipped binding closed.
    fn prepare(&mut self, _run_dir: &Path) -> Result<Vec<String>, String> {
        self.fence
            .as_ref()
            .map(|_| Vec::new())
            .map_err(ToString::to_string)
    }

    fn log_path(&self, output_file: &Path) -> PathBuf {
        output_file.with_file_name(crate::subagent::LOG_FILE_NAME)
    }

    /// Never authenticate in a run: sign-in is interactive and belongs to
    /// `/login claude-code`. A signed-out Claude fails the prompt with
    /// "Authentication required".
    fn auth_method(&self, _advertised: &[AuthMethod]) -> Option<AuthMethodId> {
        None
    }

    fn session_mode(&self) -> Option<SessionModeId> {
        self.fence()
            .map(|fence| SessionModeId::new(fence.session_mode()))
    }

    fn session_meta(&self) -> Option<Meta> {
        self.fence().map(|fence| self.meta(fence))
    }

    /// Claude asks only outside its own approvals: in Plan and `default`
    /// that is outside the fence, and under `bypassPermissions` it is a
    /// bypass-immune safety check or ask rule. `claude -p` denied those too.
    fn decide_permission(&self, _request: &RequestPermissionRequest) -> PermissionDecision {
        PermissionDecision::Reject
    }

    /// Rho advertises no elicitation, so the adapter sends no requests Rho
    /// can answer.
    fn answer_extension(&self, _method: &str, _params: &Value) -> Option<ExtensionAnswer> {
        None
    }

    fn rate_limit_recorder(&self) -> Option<Box<dyn RateLimitRecorder>> {
        Some(Box::new(ClaudeRateLimitRecorder::new(
            self.rate_limit_state_path.clone(),
        )))
    }

    /// Usage updates carry the SDK's rate-limit event and the concrete model
    /// an alias such as `opus` resolved to.
    fn status_effects(&self, update: &SessionUpdate) -> Vec<StreamEffect> {
        let SessionUpdate::UsageUpdate(usage) = update else {
            return Vec::new();
        };
        let Some(meta) = usage.meta.as_ref() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        if let Some(info) = meta.get(RATE_LIMIT_META).and_then(rate_limit_info) {
            effects.push(StreamEffect::RateLimit(info));
        }
        if let Some(model) = meta.get(MODEL_META).and_then(Value::as_str) {
            effects.push(StreamEffect::Status(StatusPatch {
                runtime_model: Some(model.to_owned()),
                ..StatusPatch::default()
            }));
        }
        effects
    }
}

/// The SDK's rate-limit event: one window at the top level, with every
/// window's utilization under `unifiedWindows`.
fn rate_limit_info(event: &Value) -> Option<RateLimitInfo> {
    let mut info: RateLimitInfo = serde_json::from_value(event.clone()).ok()?;
    if info.utilization.is_none() {
        info.utilization = event
            .get("unifiedWindows")
            .and_then(|windows| windows.get(info.window_key()))
            .and_then(|window| window.get("utilization"))
            .and_then(Value::as_f64);
    }
    Some(info)
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
