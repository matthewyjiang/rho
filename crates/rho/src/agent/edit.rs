//! Draft mutation and validated save for user-defined agent files.
//!
//! Mutators live on [`AgentDefinition`] so the TUI editor does not re-encode
//! runtime-axis invariants. Save serializes, re-parses, and replaces the file
//! only when the on-disk contents still match the edit session baseline.

use std::{
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};

use super::{
    parse_definition, parse_tools_list_text, serialize_definition, AgentDefinition, AgentRuntime,
    AgentRuntimeSpec, ClaudeAgentConfig, ClaudeToolPolicy, CursorAgentConfig, CursorTool,
    ModelPolicy, ModelSelection, PromptPolicy, ReasoningLevel, ToolCapability, ToolPolicy,
};

impl AgentDefinition {
    /// Switches prompt policy while preserving the existing body.
    pub(crate) fn set_prompt_policy_kind(&mut self, value: &str) -> bool {
        let body = match &self.prompt {
            PromptPolicy::Extend(text) | PromptPolicy::Replace(text) => text.clone(),
        };
        self.prompt = match value {
            "extend" => PromptPolicy::Extend(body),
            "replace" => PromptPolicy::Replace(body),
            _ => return false,
        };
        true
    }

    /// Replaces the prompt body while preserving extend/replace.
    pub(crate) fn set_prompt_body(&mut self, body: String) {
        self.prompt = match &self.prompt {
            PromptPolicy::Extend(_) => PromptPolicy::Extend(body),
            PromptPolicy::Replace(_) => PromptPolicy::Replace(body),
        };
    }

    /// Switches runtime, carrying compatible model/reasoning and resetting the rest.
    ///
    /// Switching to Cursor forces `prompt: extend` because Cursor cannot replace
    /// its system prompt.
    pub(crate) fn switch_runtime_kind(&mut self, value: &str) -> bool {
        let Ok(next) = value.parse::<AgentRuntime>() else {
            return false;
        };
        if self.runtime.runtime() == next {
            return true;
        }
        let reasoning = self.reasoning();
        let model_policy = self.model_policy().into_owned();
        self.runtime = build_runtime_spec(next, &model_policy, reasoning);
        if next == AgentRuntime::Cursor {
            let _ = self.set_prompt_policy_kind("extend");
        }
        true
    }

    /// Applies a model-policy keyword for the current runtime.
    pub(crate) fn set_model_policy_kind(&mut self, value: &str) -> bool {
        let runtime = self.runtime.runtime();
        let policy = match (runtime, value) {
            (_, "inherit") => ModelPolicy::Inherit,
            (AgentRuntime::Rho, "prefer") => ModelPolicy::Prefer(self.current_selection()),
            (AgentRuntime::Rho, "require") => ModelPolicy::Require(self.current_selection()),
            (_, "select") if runtime == AgentRuntime::Rho || runtime.is_external_cli() => {
                ModelPolicy::Select(self.current_selection())
            }
            _ => return false,
        };
        self.set_model_policy(policy);
        true
    }

    pub(crate) fn current_selection(&self) -> ModelSelection {
        self.model_policy()
            .selection()
            .cloned()
            .unwrap_or(ModelSelection {
                provider: None,
                model: String::new(),
                auth: None,
            })
    }

    pub(crate) fn set_reasoning_kind(&mut self, value: &str) -> bool {
        let level = if value == "inherit" {
            None
        } else {
            match value.parse::<ReasoningLevel>() {
                Ok(level) => Some(level),
                Err(_) => return false,
            }
        };
        self.set_reasoning(level);
        true
    }

    pub(crate) fn set_inherit_claude_config(&mut self, value: &str) -> bool {
        let inherit = match value {
            "yes" => true,
            "no" => false,
            _ => return false,
        };
        match &mut self.runtime {
            AgentRuntimeSpec::ClaudeCli(config) => {
                config.inherit_claude_config = inherit;
                true
            }
            AgentRuntimeSpec::Rho { .. } | AgentRuntimeSpec::Cursor(_) => false,
        }
    }

    pub(crate) fn set_description_text(&mut self, value: String) {
        self.description = value;
    }

    pub(crate) fn set_model_text(&mut self, value: String) {
        let trimmed = value.trim().to_string();
        if let Some(model) = self.runtime.pass_through_model_mut() {
            *model = (!trimmed.is_empty()).then_some(trimmed);
            return;
        }
        if trimmed.is_empty() {
            self.set_model_policy(ModelPolicy::Inherit);
            return;
        }
        let policy = self
            .model_policy()
            .into_owned()
            .map_selection(|mut selection| {
                selection.model = trimmed.clone();
                selection
            })
            .unwrap_or(ModelPolicy::Select(ModelSelection {
                provider: None,
                model: trimmed,
                auth: None,
            }));
        self.set_model_policy(policy);
    }

    pub(crate) fn set_provider_text(&mut self, value: String) {
        let trimmed = value.trim().to_string();
        let provider = (!trimmed.is_empty()).then_some(trimmed);
        let Some(policy) = self
            .model_policy()
            .into_owned()
            .map_selection(|mut selection| {
                if let Some(provider) = provider.as_deref() {
                    selection.auth = selection.auth.filter(|auth| {
                        rho_providers::provider::provider_accepts_auth(provider, auth)
                    });
                }
                selection.provider = provider;
                selection
            })
        else {
            return;
        };
        self.set_model_policy(policy);
    }

    /// Pins or clears the auth profile for a non-inherit model policy.
    ///
    /// When setting an auth profile and provider is empty or incompatible, the
    /// provider is updated from the auth profile's provider.
    pub(crate) fn set_auth_selection(&mut self, auth: Option<String>) -> bool {
        if self.runtime.runtime().is_external_cli() {
            return auth.is_none();
        }
        let resolved = match auth {
            None => None,
            Some(auth) => {
                let Some((descriptor, mode)) = rho_providers::provider::resolve_auth_mode(&auth)
                else {
                    return false;
                };
                Some((descriptor.name, mode.id))
            }
        };
        let Some(policy) = self
            .model_policy()
            .into_owned()
            .map_selection(|mut selection| {
                match resolved {
                    None => selection.auth = None,
                    Some((provider_name, auth_id)) => {
                        let keep_provider = selection.provider.as_deref().is_some_and(|provider| {
                            rho_providers::provider::provider_accepts_auth(provider, auth_id)
                        });
                        if !keep_provider {
                            selection.provider = Some(provider_name.to_string());
                        }
                        selection.auth = Some(auth_id.to_string());
                    }
                }
                selection
            })
        else {
            return false;
        };
        self.set_model_policy(policy);
        true
    }

    pub(crate) fn set_model_selection(&mut self, selection: Option<ModelSelection>) {
        if let Some(model) = self.runtime.pass_through_model_mut() {
            *model = selection
                .map(|value| value.model)
                .filter(|value| !value.is_empty());
            return;
        }
        let policy = match (self.model_policy().as_ref(), selection) {
            (ModelPolicy::Prefer(_), Some(selection)) if !selection.model.is_empty() => {
                ModelPolicy::Prefer(selection)
            }
            (ModelPolicy::Require(_), Some(selection)) if !selection.model.is_empty() => {
                ModelPolicy::Require(selection)
            }
            (_, Some(selection)) if !selection.model.is_empty() => ModelPolicy::Select(selection),
            _ => ModelPolicy::Inherit,
        };
        self.set_model_policy(policy);
    }

    pub(crate) fn set_model_policy(&mut self, policy: ModelPolicy) {
        if let Some(model) = self.runtime.pass_through_model_mut() {
            *model = match policy {
                ModelPolicy::Inherit => None,
                ModelPolicy::Prefer(selection)
                | ModelPolicy::Require(selection)
                | ModelPolicy::Select(selection) => Some(selection.model),
            };
            return;
        }
        match &mut self.runtime {
            AgentRuntimeSpec::Rho { model, .. } => *model = policy,
            AgentRuntimeSpec::ClaudeCli(_) | AgentRuntimeSpec::Cursor(_) => {
                unreachable!("external CLI runtimes expose a pass-through model")
            }
        }
    }

    pub(crate) fn set_reasoning(&mut self, level: Option<ReasoningLevel>) {
        match &mut self.runtime {
            AgentRuntimeSpec::Rho { reasoning, .. } => *reasoning = level,
            AgentRuntimeSpec::ClaudeCli(config) => {
                config.reasoning = level.filter(|level| {
                    !matches!(level, ReasoningLevel::Off | ReasoningLevel::Minimal)
                });
            }
            AgentRuntimeSpec::Cursor(_) => {}
        }
    }

    /// Parses tools text (`all`, `[]`, or a bracket list) into the runtime policy.
    pub(crate) fn set_tools_text(&mut self, value: &str) -> Result<(), String> {
        let trimmed = value.trim();
        match &mut self.runtime {
            AgentRuntimeSpec::Rho { tools, .. } => {
                if trimmed == "all" {
                    *tools = ToolPolicy::All;
                    return Ok(());
                }
                let names = parse_tools_list_text(trimmed)?;
                let mut capabilities = std::collections::BTreeSet::new();
                for name in names {
                    capabilities.insert(ToolCapability::parse(name));
                }
                *tools = ToolPolicy::Allow(capabilities);
                Ok(())
            }
            AgentRuntimeSpec::ClaudeCli(config) => {
                let names = parse_tools_list_text(trimmed)?;
                config.tools = if names.is_empty() {
                    ClaudeToolPolicy::None
                } else {
                    ClaudeToolPolicy::Allow(names)
                };
                Ok(())
            }
            AgentRuntimeSpec::Cursor(config) => {
                if trimmed == "all" {
                    return Err(
                        "runtime: cursor does not support tools: all; list closed snake_case names"
                            .into(),
                    );
                }
                let names = parse_tools_list_text(trimmed)?;
                if names.is_empty() {
                    return Err("cursor agents need at least one tool".into());
                }
                let mut tools = Vec::with_capacity(names.len());
                for name in names {
                    tools.push(CursorTool::from_str(&name).map_err(|error| error.to_string())?);
                }
                config.tools = tools;
                Ok(())
            }
        }
    }

    pub(crate) fn tools_text(&self) -> String {
        match &self.runtime {
            AgentRuntimeSpec::Rho {
                tools: ToolPolicy::All,
                ..
            } => "all".into(),
            AgentRuntimeSpec::Rho {
                tools: ToolPolicy::Allow(tools),
                ..
            } => format!(
                "[{}]",
                tools
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AgentRuntimeSpec::ClaudeCli(config) => match &config.tools {
                ClaudeToolPolicy::None => "[]".into(),
                ClaudeToolPolicy::Allow(tools) => format!("[{}]", tools.join(", ")),
            },
            AgentRuntimeSpec::Cursor(config) => format!(
                "[{}]",
                config
                    .tools
                    .iter()
                    .map(|tool| tool.as_flag())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    pub(crate) fn model_text(&self) -> String {
        self.model_policy()
            .selection()
            .map(|selection| selection.model.clone())
            .unwrap_or_default()
    }

    pub(crate) fn provider_text(&self) -> String {
        self.model_policy()
            .selection()
            .and_then(|selection| selection.provider.clone())
            .unwrap_or_default()
    }

    pub(crate) fn auth_text(&self) -> String {
        self.model_policy()
            .selection()
            .and_then(|selection| selection.auth.clone())
            .unwrap_or_default()
    }

    pub(crate) fn auth_badge(&self) -> String {
        match self.auth_text() {
            value if value.is_empty() => "host".into(),
            value => value,
        }
    }

    pub(crate) fn tools_badge(&self) -> String {
        match &self.runtime {
            AgentRuntimeSpec::Rho {
                tools: ToolPolicy::All,
                ..
            } => "all".into(),
            AgentRuntimeSpec::Rho {
                tools: ToolPolicy::Allow(tools),
                ..
            } if tools.is_empty() => "none".into(),
            AgentRuntimeSpec::Rho {
                tools: ToolPolicy::Allow(tools),
                ..
            } => tools
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            AgentRuntimeSpec::ClaudeCli(config) => {
                let tools = config.tools.as_slice();
                if tools.is_empty() {
                    "none".into()
                } else {
                    tools.join(", ")
                }
            }
            AgentRuntimeSpec::Cursor(config) if config.tools.is_empty() => "none".into(),
            AgentRuntimeSpec::Cursor(config) => config
                .tools
                .iter()
                .map(|tool| tool.as_flag())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    pub(crate) fn model_badge(&self) -> String {
        match self.model_policy().as_ref() {
            ModelPolicy::Inherit => "inherit".into(),
            ModelPolicy::Prefer(selection) => format!("prefer {}", selection.model),
            ModelPolicy::Require(selection) => format!("require {}", selection.model),
            ModelPolicy::Select(selection) => selection.model.clone(),
        }
    }

    pub(crate) fn model_policy_badge(&self) -> String {
        match self.model_policy().as_ref() {
            ModelPolicy::Inherit => "inherit".into(),
            ModelPolicy::Prefer(_) => "prefer".into(),
            ModelPolicy::Require(_) => "require".into(),
            ModelPolicy::Select(_) => "select".into(),
        }
    }

    /// Friendly pre-checks for constraints the parser also enforces.
    pub(crate) fn validate_for_edit(&self) -> Option<String> {
        if self.description.chars().count() > 1024 {
            return Some("description must be at most 1024 characters".into());
        }
        if self.description.trim().is_empty() {
            return Some("description is required".into());
        }
        if let PromptPolicy::Replace(body) = &self.prompt {
            if body.trim().is_empty() {
                return Some("prompt policy 'replace' requires a non-empty prompt body".into());
            }
        }
        match &self.runtime {
            AgentRuntimeSpec::Cursor(config) if config.tools.is_empty() => {
                Some("cursor agents need at least one tool".into())
            }
            AgentRuntimeSpec::Cursor(_) => {
                if matches!(self.prompt, PromptPolicy::Replace(_)) {
                    Some("cursor cannot replace its system prompt; use extend".into())
                } else {
                    None
                }
            }
            AgentRuntimeSpec::Rho { .. } | AgentRuntimeSpec::ClaudeCli(_) => None,
        }
    }
}

fn build_runtime_spec(
    runtime: AgentRuntime,
    model_policy: &ModelPolicy,
    reasoning: Option<ReasoningLevel>,
) -> AgentRuntimeSpec {
    match runtime {
        AgentRuntime::Rho => {
            let model = match model_policy.selection() {
                Some(selection) => ModelPolicy::Select(selection.clone()),
                None => ModelPolicy::Inherit,
            };
            AgentRuntimeSpec::Rho {
                tools: ToolPolicy::All,
                model,
                reasoning,
            }
        }
        AgentRuntime::ClaudeCli => {
            let reasoning = reasoning
                .filter(|level| !matches!(level, ReasoningLevel::Off | ReasoningLevel::Minimal));
            let model = model_policy
                .selection()
                .map(|selection| selection.model.clone());
            AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
                tools: ClaudeToolPolicy::None,
                inherit_claude_config: false,
                model,
                reasoning,
            })
        }
        AgentRuntime::Cursor => {
            let model = model_policy
                .selection()
                .map(|selection| selection.model.clone());
            AgentRuntimeSpec::Cursor(CursorAgentConfig {
                tools: Vec::new(),
                model,
            })
        }
    }
}

/// Saves a draft when the file still matches `original_contents`.
pub(crate) fn save_definition(
    draft: &AgentDefinition,
    path: &Path,
    original_contents: &str,
) -> Result<String, SaveDefinitionError> {
    let contents = canonical_definition_contents(draft, path)?;
    let _lock = acquire_agent_file_lock(path)?;
    let current = read_current_agent_file(path)?.unwrap_or_default();
    if current != original_contents {
        return Err(SaveDefinitionError::Conflict);
    }
    write_agent_file(path, contents.as_bytes())?;
    Ok(contents)
}

pub(super) fn canonical_definition_contents(
    draft: &AgentDefinition,
    path: &Path,
) -> Result<String, SaveDefinitionError> {
    let contents = serialize_definition(draft);
    if let Err(error) = parse_definition(path, draft.id.as_str(), &contents) {
        return Err(SaveDefinitionError::Validation(error.to_string()));
    }
    Ok(contents)
}

pub(super) fn agent_lock_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.rho-edit.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent")
    ))
}

/// Exclusive sidecar lock for one agent file. Drop unlocks; it never unlinks
/// the lock path, so concurrent openers keep one identity.
pub(super) fn acquire_agent_file_lock(path: &Path) -> Result<AgentFileLock, SaveDefinitionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| SaveDefinitionError::Write(error.to_string()))?;
    }
    let lock_path = agent_lock_path(path);
    let mut lock_options = fs::OpenOptions::new();
    lock_options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = lock_options.open(&lock_path).map_err(|error| {
        SaveDefinitionError::Write(format!("could not open edit lock: {error}"))
    })?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
        SaveDefinitionError::Write(format!("could not lock agent file: {error}"))
    })?;
    Ok(AgentFileLock { file })
}

pub(super) fn read_current_agent_file(path: &Path) -> Result<Option<String>, SaveDefinitionError> {
    match fs::read_to_string(path) {
        Ok(current) => Ok(Some(current)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SaveDefinitionError::Write(error.to_string())),
    }
}

pub(super) fn write_agent_file(path: &Path, contents: &[u8]) -> Result<(), SaveDefinitionError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            SaveDefinitionError::Write("destination is not a regular file".into()),
        ),
        Ok(_) => crate::config_writer::replace_regular_file_atomically(path, contents)
            .map_err(|error| SaveDefinitionError::Write(error.to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            crate::config_writer::write_bytes_atomically(path, contents)
                .map_err(|error| SaveDefinitionError::Write(error.to_string()))
        }
        Err(error) => Err(SaveDefinitionError::Write(error.to_string())),
    }
}

pub(super) struct AgentFileLock {
    file: std::fs::File,
}

impl Drop for AgentFileLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveDefinitionError {
    Validation(String),
    Conflict,
    Write(String),
}

impl std::fmt::Display for SaveDefinitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(message) => write!(formatter, "agent validation failed: {message}"),
            Self::Conflict => write!(formatter, "agent file changed since editing began"),
            Self::Write(message) => write!(formatter, "could not write agent file: {message}"),
        }
    }
}

impl std::error::Error for SaveDefinitionError {}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
