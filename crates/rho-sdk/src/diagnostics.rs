use std::path::{Path, PathBuf};

use crate::{
    model::ModelIdentity,
    tool::{ToolOrigin, ToolSecurity},
    ApprovalAuditRecord, CapabilityKind,
};

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

/// Secret-free declaration for one registered tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDiagnostic {
    name: String,
    origin: ToolOrigin,
    capabilities: Vec<CapabilityKind>,
}

impl ToolDiagnostic {
    pub(crate) fn new(name: String, security: ToolSecurity) -> Self {
        Self {
            name,
            origin: security.origin(),
            capabilities: security.capabilities().to_vec(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn origin(&self) -> ToolOrigin {
        self.origin
    }

    pub fn capabilities(&self) -> &[CapabilityKind] {
        &self.capabilities
    }
}

/// Stable snapshot of effective runtime configuration without secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticsSnapshot {
    provider: ModelIdentity,
    tool_names: Vec<String>,
    tools: Vec<ToolDiagnostic>,
    workspace_root: Option<PathBuf>,
    granted_workspace_roots: Vec<PathBuf>,
    prompt_sources: Vec<PromptSource>,
    approval_audit: Vec<ApprovalAuditRecord>,
    event_capacity: usize,
    max_steps: usize,
    max_parallel_tools: usize,
    compaction_trigger_messages: Option<usize>,
    compaction_trigger_tokens: Option<u64>,
    reasoning_level: crate::ReasoningLevel,
    default_features: Vec<String>,
    usage_recorder_diagnostics: Vec<crate::UsageRecorderDiagnostic>,
}

pub(crate) struct SecuritySettings {
    pub(crate) tool_names: Vec<String>,
    pub(crate) tool_security: Vec<(String, ToolSecurity)>,
    pub(crate) workspace_root: Option<PathBuf>,
    pub(crate) granted_workspace_roots: Vec<PathBuf>,
    pub(crate) prompt_sources: Vec<PromptSource>,
    pub(crate) approval_audit: Vec<ApprovalAuditRecord>,
}

pub(crate) struct ExecutionSettings {
    pub(crate) event_capacity: usize,
    pub(crate) max_steps: usize,
    pub(crate) max_parallel_tools: usize,
    pub(crate) compaction_trigger_messages: Option<usize>,
    pub(crate) compaction_trigger_tokens: Option<u64>,
    pub(crate) reasoning_level: crate::ReasoningLevel,
    pub(crate) usage_recorder_diagnostics: Vec<crate::UsageRecorderDiagnostic>,
}

impl DiagnosticsSnapshot {
    pub(crate) fn new(
        provider: ModelIdentity,
        security: SecuritySettings,
        execution: ExecutionSettings,
    ) -> Self {
        let tools = security
            .tool_security
            .into_iter()
            .map(|(name, security)| ToolDiagnostic::new(name, security))
            .collect();
        Self {
            provider,
            tool_names: security.tool_names,
            tools,
            workspace_root: security.workspace_root,
            granted_workspace_roots: security.granted_workspace_roots,
            prompt_sources: security.prompt_sources,
            approval_audit: security.approval_audit,
            event_capacity: execution.event_capacity,
            max_steps: execution.max_steps,
            max_parallel_tools: execution.max_parallel_tools,
            compaction_trigger_messages: execution.compaction_trigger_messages,
            compaction_trigger_tokens: execution.compaction_trigger_tokens,
            reasoning_level: execution.reasoning_level,
            default_features: Vec::new(),
            usage_recorder_diagnostics: execution.usage_recorder_diagnostics,
        }
    }

    pub fn provider(&self) -> &ModelIdentity {
        &self.provider
    }

    pub fn tool_names(&self) -> &[String] {
        &self.tool_names
    }

    pub fn tools(&self) -> &[ToolDiagnostic] {
        &self.tools
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    pub fn granted_workspace_roots(&self) -> &[PathBuf] {
        &self.granted_workspace_roots
    }

    pub fn prompt_sources(&self) -> &[PromptSource] {
        &self.prompt_sources
    }

    pub fn approval_audit(&self) -> &[ApprovalAuditRecord] {
        &self.approval_audit
    }

    pub fn event_capacity(&self) -> usize {
        self.event_capacity
    }

    pub fn max_steps(&self) -> usize {
        self.max_steps
    }

    pub fn max_parallel_tools(&self) -> usize {
        self.max_parallel_tools
    }

    pub fn compaction_trigger_messages(&self) -> Option<usize> {
        self.compaction_trigger_messages
    }

    pub fn compaction_trigger_tokens(&self) -> Option<u64> {
        self.compaction_trigger_tokens
    }

    pub fn reasoning_level(&self) -> crate::ReasoningLevel {
        self.reasoning_level
    }

    pub fn usage_recorder_diagnostics(&self) -> &[crate::UsageRecorderDiagnostic] {
        &self.usage_recorder_diagnostics
    }

    /// Enabled capability feature labels. Empty for the minimal default SDK.
    pub fn enabled_features(&self) -> &[String] {
        &self.default_features
    }
}
