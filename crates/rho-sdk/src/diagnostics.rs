use std::path::{Path, PathBuf};

use crate::model::ModelIdentity;

/// Kind of source included in the effective system prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PromptSourceKind {
    BuiltIn,
    Custom,
    WorkspaceInstruction,
    Skill,
}

/// Inspectable source included in prompt construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptSource {
    kind: PromptSourceKind,
    label: String,
    path: Option<PathBuf>,
}

impl PromptSource {
    pub(crate) fn custom() -> Self {
        Self {
            kind: PromptSourceKind::Custom,
            label: "custom system prompt".into(),
            path: None,
        }
    }

    pub fn kind(&self) -> PromptSourceKind {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// Stable snapshot of effective runtime configuration without secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticsSnapshot {
    provider: ModelIdentity,
    tool_names: Vec<String>,
    workspace_root: Option<PathBuf>,
    prompt_sources: Vec<PromptSource>,
    event_capacity: usize,
    max_steps: usize,
    compaction_trigger_messages: Option<usize>,
    reasoning_level: crate::ReasoningLevel,
    default_features: Vec<String>,
}

pub(crate) struct ExecutionSettings {
    pub(crate) event_capacity: usize,
    pub(crate) max_steps: usize,
    pub(crate) compaction_trigger_messages: Option<usize>,
    pub(crate) reasoning_level: crate::ReasoningLevel,
}

impl DiagnosticsSnapshot {
    pub(crate) fn new(
        provider: ModelIdentity,
        tool_names: Vec<String>,
        workspace_root: Option<PathBuf>,
        prompt_sources: Vec<PromptSource>,
        execution: ExecutionSettings,
    ) -> Self {
        Self {
            provider,
            tool_names,
            workspace_root,
            prompt_sources,
            event_capacity: execution.event_capacity,
            max_steps: execution.max_steps,
            compaction_trigger_messages: execution.compaction_trigger_messages,
            reasoning_level: execution.reasoning_level,
            default_features: Vec::new(),
        }
    }

    pub fn provider(&self) -> &ModelIdentity {
        &self.provider
    }

    pub fn tool_names(&self) -> &[String] {
        &self.tool_names
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    pub fn prompt_sources(&self) -> &[PromptSource] {
        &self.prompt_sources
    }

    pub fn event_capacity(&self) -> usize {
        self.event_capacity
    }

    pub fn max_steps(&self) -> usize {
        self.max_steps
    }

    pub fn compaction_trigger_messages(&self) -> Option<usize> {
        self.compaction_trigger_messages
    }

    pub fn reasoning_level(&self) -> crate::ReasoningLevel {
        self.reasoning_level
    }

    /// Enabled capability feature labels. Empty for the minimal default SDK.
    pub fn enabled_features(&self) -> &[String] {
        &self.default_features
    }
}
