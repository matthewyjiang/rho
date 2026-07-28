//! Draft mutation and validated save for user-defined agent files.
//!
//! Mutators live on [`AgentDefinition`] so the TUI editor does not re-encode
//! runtime-axis invariants. Save serializes, re-parses, and replaces the file
//! only when the on-disk contents still match the edit session baseline.

use std::path::Path;

use super::{
    parse_definition, parse_tools_list_text, serialize_definition, AgentDefinition, AgentRuntime,
    AgentRuntimeSpec, ClaudeAgentConfig, ClaudeToolPolicy, ModelPolicy, ModelSelection,
    PromptPolicy, ReasoningLevel, ToolCapability, ToolPolicy,
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
    pub(crate) fn switch_runtime_kind(&mut self, value: &str) -> bool {
        let next = match value {
            "rho" => AgentRuntime::Rho,
            "claude-cli" => AgentRuntime::ClaudeCli,
            _ => return false,
        };
        if self.runtime.runtime() == next {
            return true;
        }
        let reasoning = self.reasoning();
        let model_policy = self.model_policy().into_owned();
        self.runtime = build_runtime_spec(next, &model_policy, reasoning);
        true
    }

    /// Applies a model-policy keyword for the current runtime.
    pub(crate) fn set_model_policy_kind(&mut self, value: &str) -> bool {
        let is_claude = self.runtime.runtime() == AgentRuntime::ClaudeCli;
        let policy = match value {
            "inherit" => {
                self.set_model_selection(None, None);
                ModelPolicy::Inherit
            }
            "prefer" if !is_claude => ModelPolicy::Prefer(self.current_selection()),
            "require" if !is_claude => ModelPolicy::Require(self.current_selection()),
            "select" => ModelPolicy::Select(self.current_selection()),
            _ => return false,
        };
        self.set_model_policy(policy);
        true
    }

    pub(crate) fn current_selection(&self) -> ModelSelection {
        match self.model_policy().as_ref() {
            ModelPolicy::Prefer(selection)
            | ModelPolicy::Require(selection)
            | ModelPolicy::Select(selection) => ModelSelection {
                provider: selection.provider.clone(),
                model: selection.model.clone(),
            },
            ModelPolicy::Inherit => ModelSelection {
                provider: None,
                model: String::new(),
            },
        }
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
        if let AgentRuntimeSpec::ClaudeCli(config) = &mut self.runtime {
            config.inherit_claude_config = inherit;
            true
        } else {
            false
        }
    }

    pub(crate) fn set_description_text(&mut self, value: String) {
        self.description = value;
    }

    pub(crate) fn set_model_text(&mut self, value: String) {
        let trimmed = value.trim().to_string();
        if self.runtime.runtime() == AgentRuntime::ClaudeCli {
            if let AgentRuntimeSpec::ClaudeCli(config) = &mut self.runtime {
                config.model = (!trimmed.is_empty()).then_some(trimmed);
            }
            return;
        }
        if trimmed.is_empty() {
            self.set_model_policy(ModelPolicy::Inherit);
            return;
        }
        let provider = match self.model_policy().as_ref() {
            ModelPolicy::Prefer(selection)
            | ModelPolicy::Require(selection)
            | ModelPolicy::Select(selection) => selection.provider.clone(),
            ModelPolicy::Inherit => None,
        };
        let policy = match self.model_policy().as_ref() {
            ModelPolicy::Prefer(_) => ModelPolicy::Prefer(ModelSelection {
                provider,
                model: trimmed,
            }),
            ModelPolicy::Require(_) => ModelPolicy::Require(ModelSelection {
                provider,
                model: trimmed,
            }),
            _ => ModelPolicy::Select(ModelSelection {
                provider,
                model: trimmed,
            }),
        };
        self.set_model_policy(policy);
    }

    pub(crate) fn set_provider_text(&mut self, value: String) {
        let trimmed = value.trim().to_string();
        let model = match self.model_policy().as_ref() {
            ModelPolicy::Prefer(selection)
            | ModelPolicy::Require(selection)
            | ModelPolicy::Select(selection) => selection.model.clone(),
            ModelPolicy::Inherit => return,
        };
        let provider = (!trimmed.is_empty()).then_some(trimmed);
        let policy = match self.model_policy().as_ref() {
            ModelPolicy::Prefer(_) => ModelPolicy::Prefer(ModelSelection { provider, model }),
            ModelPolicy::Require(_) => ModelPolicy::Require(ModelSelection { provider, model }),
            _ => ModelPolicy::Select(ModelSelection { provider, model }),
        };
        self.set_model_policy(policy);
    }

    pub(crate) fn set_model_selection(&mut self, provider: Option<String>, model: Option<String>) {
        if self.runtime.runtime() == AgentRuntime::ClaudeCli {
            if let AgentRuntimeSpec::ClaudeCli(config) = &mut self.runtime {
                config.model = model.filter(|value| !value.is_empty());
            }
            return;
        }
        let policy = match (self.model_policy().as_ref(), model) {
            (ModelPolicy::Prefer(_), Some(model)) if !model.is_empty() => {
                ModelPolicy::Prefer(ModelSelection { provider, model })
            }
            (ModelPolicy::Require(_), Some(model)) if !model.is_empty() => {
                ModelPolicy::Require(ModelSelection { provider, model })
            }
            (_, Some(model)) if !model.is_empty() => {
                ModelPolicy::Select(ModelSelection { provider, model })
            }
            _ => ModelPolicy::Inherit,
        };
        self.set_model_policy(policy);
    }

    pub(crate) fn set_model_policy(&mut self, policy: ModelPolicy) {
        match &mut self.runtime {
            AgentRuntimeSpec::Rho { model, .. } => *model = policy,
            AgentRuntimeSpec::ClaudeCli(config) => {
                config.model = match policy {
                    ModelPolicy::Inherit => None,
                    ModelPolicy::Prefer(selection)
                    | ModelPolicy::Require(selection)
                    | ModelPolicy::Select(selection) => Some(selection.model),
                };
            }
        }
    }

    pub(crate) fn set_reasoning(&mut self, level: Option<ReasoningLevel>) {
        let level = if self.runtime.runtime() == AgentRuntime::ClaudeCli {
            level.filter(|level| !matches!(level, ReasoningLevel::Off | ReasoningLevel::Minimal))
        } else {
            level
        };
        match &mut self.runtime {
            AgentRuntimeSpec::Rho { reasoning, .. } => *reasoning = level,
            AgentRuntimeSpec::ClaudeCli(config) => config.reasoning = level,
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
        }
    }

    pub(crate) fn model_text(&self) -> String {
        match self.model_policy().as_ref() {
            ModelPolicy::Inherit => String::new(),
            ModelPolicy::Prefer(selection)
            | ModelPolicy::Require(selection)
            | ModelPolicy::Select(selection) => selection.model.clone(),
        }
    }

    pub(crate) fn provider_text(&self) -> String {
        match self.model_policy().as_ref() {
            ModelPolicy::Prefer(selection)
            | ModelPolicy::Require(selection)
            | ModelPolicy::Select(selection) => selection.provider.clone().unwrap_or_default(),
            ModelPolicy::Inherit => String::new(),
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
        None
    }
}

fn build_runtime_spec(
    runtime: AgentRuntime,
    model_policy: &ModelPolicy,
    reasoning: Option<ReasoningLevel>,
) -> AgentRuntimeSpec {
    match runtime {
        AgentRuntime::Rho => {
            let model = match model_policy {
                ModelPolicy::Inherit => ModelPolicy::Inherit,
                ModelPolicy::Prefer(selection)
                | ModelPolicy::Require(selection)
                | ModelPolicy::Select(selection) => ModelPolicy::Select(ModelSelection {
                    provider: selection.provider.clone(),
                    model: selection.model.clone(),
                }),
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
            let model = match model_policy {
                ModelPolicy::Inherit => None,
                ModelPolicy::Prefer(selection)
                | ModelPolicy::Require(selection)
                | ModelPolicy::Select(selection) => Some(selection.model.clone()),
            };
            AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
                tools: ClaudeToolPolicy::None,
                inherit_claude_config: false,
                model,
                reasoning,
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
    let contents = serialize_definition(draft);
    if let Err(error) = parse_definition(path, draft.id.as_str(), &contents) {
        return Err(SaveDefinitionError::Validation(error.to_string()));
    }

    let lock_path = path.with_file_name(format!(
        ".{}.rho-edit.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent")
    ));
    let mut lock_options = std::fs::OpenOptions::new();
    lock_options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.custom_flags(libc::O_NOFOLLOW);
    }
    let lock_file = lock_options.open(&lock_path).map_err(|error| {
        SaveDefinitionError::Write(format!("could not open edit lock: {error}"))
    })?;
    let _lock_guard = FileLockGuard {
        file: lock_file,
        path: lock_path,
    };
    fs2::FileExt::try_lock_exclusive(&_lock_guard.file).map_err(|error| {
        SaveDefinitionError::Write(format!("could not lock agent file: {error}"))
    })?;

    let current_contents = std::fs::read_to_string(path)
        .map_err(|error| SaveDefinitionError::Write(error.to_string()))?;
    if current_contents != original_contents {
        return Err(SaveDefinitionError::Conflict);
    }
    crate::config_writer::replace_regular_file_atomically(path, contents.as_bytes())
        .map_err(|error| SaveDefinitionError::Write(error.to_string()))?;
    Ok(contents)
}

struct FileLockGuard {
    file: std::fs::File,
    path: std::path::PathBuf,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        let _ = std::fs::remove_file(&self.path);
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
