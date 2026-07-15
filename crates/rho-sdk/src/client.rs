use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use crate::{
    model::Message,
    persistence::SessionSnapshot,
    provider::ModelProvider,
    session::{Session, SessionCore},
    tool::{Tool, ToolRegistry},
    Error, SessionId,
};

const DEFAULT_EVENT_CAPACITY: usize = 64;
const DEFAULT_MAX_STEPS: usize = 32;

#[derive(Debug, Default)]
struct LifecycleState {
    shutdown: bool,
    runs: BTreeMap<crate::RunId, crate::CancellationToken>,
}

#[derive(Debug, Default)]
pub(crate) struct RuntimeLifecycle {
    state: Mutex<LifecycleState>,
}

impl RuntimeLifecycle {
    pub(crate) fn register(
        &self,
        run_id: crate::RunId,
        cancellation: crate::CancellationToken,
    ) -> Result<(), Error> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            return Err(Error::RuntimeShutdown);
        }
        state.runs.insert(run_id, cancellation);
        Ok(())
    }

    pub(crate) fn unregister(&self, run_id: &crate::RunId) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .runs
            .remove(run_id);
    }

    fn shutdown(&self) -> ShutdownOutcome {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            return ShutdownOutcome::default();
        }
        state.shutdown = true;
        let cancelled_runs = state.runs.len();
        for cancellation in state.runs.values() {
            cancellation.cancel();
        }
        ShutdownOutcome { cancelled_runs }
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .shutdown
    }
}

/// Result of explicitly shutting down an SDK runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShutdownOutcome {
    cancelled_runs: usize,
}

impl ShutdownOutcome {
    pub fn cancelled_runs(&self) -> usize {
        self.cancelled_runs
    }
}

/// Explicit system-prompt policy for a runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemPrompt {
    #[default]
    None,
    Custom(String),
}

/// Options used to create an in-memory SDK session.
#[derive(Clone, Debug)]
pub struct SessionOptions {
    id: SessionId,
    history: Vec<Message>,
    revision: crate::Revision,
    compaction: crate::CompactionState,
    prompt_cache_key: Option<String>,
    apply_system_prompt: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            id: SessionId::default(),
            history: Vec::new(),
            revision: crate::Revision::INITIAL,
            compaction: crate::CompactionState::default(),
            prompt_cache_key: None,
            apply_system_prompt: true,
        }
    }
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(mut self, id: SessionId) -> Self {
        self.id = id;
        self
    }

    pub fn history(mut self, history: Vec<Message>) -> Self {
        self.history = history;
        self
    }

    /// Sets an opaque, non-secret cache key forwarded to supporting providers.
    pub fn prompt_cache_key(mut self, prompt_cache_key: impl Into<String>) -> Self {
        self.prompt_cache_key = Some(prompt_cache_key.into());
        self
    }

    pub fn from_snapshot(snapshot: SessionSnapshot) -> Self {
        Self {
            id: snapshot.session_id().clone(),
            history: snapshot.history().to_vec(),
            revision: snapshot.revision(),
            compaction: snapshot.compaction().clone(),
            prompt_cache_key: None,
            apply_system_prompt: false,
        }
    }
}

/// Builder for the headless Rho runtime.
#[derive(Default)]
pub struct RhoBuilder {
    provider: Option<Arc<dyn ModelProvider>>,
    tools: Vec<Arc<dyn Tool>>,
    system_prompt: SystemPrompt,
    event_capacity: Option<NonZeroUsize>,
    max_steps: Option<NonZeroUsize>,
    workspace: Option<crate::Workspace>,
    workspace_policy: Option<Arc<dyn crate::WorkspacePolicy>>,
    approval_handler: Option<Arc<dyn crate::ApprovalHandler>>,
    compactor: Option<Arc<dyn crate::Compactor>>,
    compaction_policy: Option<crate::CompactionPolicy>,
    reasoning_level: crate::ReasoningLevel,
}

impl RhoBuilder {
    pub fn provider<P>(mut self, provider: P) -> Self
    where
        P: ModelProvider + 'static,
    {
        self.provider = Some(Arc::new(provider));
        self
    }

    pub fn provider_shared(mut self, provider: Arc<dyn ModelProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn tool<T>(mut self, tool: T) -> Self
    where
        T: Tool + 'static,
    {
        self.tools.push(Arc::new(tool));
        self
    }

    /// Registers an already shared tool implementation on the runtime.
    pub fn tool_shared(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn system_prompt(mut self, system_prompt: SystemPrompt) -> Self {
        self.system_prompt = system_prompt;
        self
    }

    pub fn event_capacity(mut self, capacity: NonZeroUsize) -> Self {
        self.event_capacity = Some(capacity);
        self
    }

    pub fn max_steps(mut self, max_steps: NonZeroUsize) -> Self {
        self.max_steps = Some(max_steps);
        self
    }

    /// Supplies an explicit filesystem scope without granting capabilities.
    pub fn workspace(mut self, workspace: crate::Workspace) -> Self {
        self.workspace = Some(workspace);
        self
    }

    pub fn workspace_policy<P>(mut self, policy: P) -> Self
    where
        P: crate::WorkspacePolicy + 'static,
    {
        self.workspace_policy = Some(Arc::new(policy));
        self
    }

    pub fn approval_handler<A>(mut self, handler: A) -> Self
    where
        A: crate::ApprovalHandler + 'static,
    {
        self.approval_handler = Some(Arc::new(handler));
        self
    }

    pub fn approval_handler_shared(mut self, handler: Arc<dyn crate::ApprovalHandler>) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    pub fn compactor<C>(mut self, compactor: C) -> Self
    where
        C: crate::Compactor + 'static,
    {
        self.compactor = Some(Arc::new(compactor));
        self
    }

    pub fn compaction_policy(mut self, policy: crate::CompactionPolicy) -> Self {
        self.compaction_policy = Some(policy);
        self
    }

    pub fn reasoning_level(mut self, reasoning_level: crate::ReasoningLevel) -> Self {
        self.reasoning_level = reasoning_level;
        self
    }

    pub fn build(self) -> Result<Rho, Error> {
        let provider = self.provider.ok_or_else(|| Error::InvalidConfiguration {
            message: "a model provider is required".into(),
        })?;
        let mut tools = ToolRegistry::new();
        for tool in self.tools {
            tools
                .register_shared(tool)
                .map_err(|error| Error::InvalidConfiguration {
                    message: error.to_string(),
                })?;
        }
        if self.compaction_policy.is_some() && self.compactor.is_none() {
            return Err(Error::InvalidConfiguration {
                message: "automatic compaction policy requires a compactor".into(),
            });
        }
        Ok(Rho {
            provider,
            tools,
            system_prompt: self.system_prompt,
            event_capacity: self
                .event_capacity
                .unwrap_or_else(|| NonZeroUsize::new(DEFAULT_EVENT_CAPACITY).unwrap()),
            max_steps: self
                .max_steps
                .unwrap_or_else(|| NonZeroUsize::new(DEFAULT_MAX_STEPS).unwrap()),
            workspace: self.workspace,
            workspace_policy: self
                .workspace_policy
                .unwrap_or_else(|| Arc::new(crate::DenyAllPolicy)),
            approval_handler: self
                .approval_handler
                .unwrap_or_else(|| Arc::new(crate::DenyApprovals)),
            compactor: self.compactor,
            compaction_policy: self.compaction_policy,
            reasoning_level: self.reasoning_level,
            approval_audit: Arc::default(),
            lifecycle: Arc::new(RuntimeLifecycle::default()),
        })
    }
}

/// Headless runtime configuration shared by SDK sessions.
#[derive(Clone)]
pub struct Rho {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) tools: ToolRegistry,
    pub(crate) system_prompt: SystemPrompt,
    pub(crate) event_capacity: NonZeroUsize,
    pub(crate) max_steps: NonZeroUsize,
    pub(crate) workspace: Option<crate::Workspace>,
    pub(crate) workspace_policy: Arc<dyn crate::WorkspacePolicy>,
    pub(crate) approval_handler: Arc<dyn crate::ApprovalHandler>,
    pub(crate) compactor: Option<Arc<dyn crate::Compactor>>,
    pub(crate) compaction_policy: Option<crate::CompactionPolicy>,
    pub(crate) reasoning_level: crate::ReasoningLevel,
    pub(crate) approval_audit: Arc<crate::workspace::ApprovalAuditLog>,
    pub(crate) lifecycle: Arc<RuntimeLifecycle>,
}

impl Rho {
    pub fn builder() -> RhoBuilder {
        RhoBuilder::default()
    }

    pub fn shutdown(&self) -> ShutdownOutcome {
        self.lifecycle.shutdown()
    }

    pub fn diagnostics(&self) -> crate::DiagnosticsSnapshot {
        let prompt_sources = match &self.system_prompt {
            SystemPrompt::None => Vec::new(),
            SystemPrompt::Custom(_) => vec![crate::diagnostics::PromptSource::custom()],
        };
        crate::DiagnosticsSnapshot::new(
            self.provider.identity(),
            crate::diagnostics::SecuritySettings {
                tool_names: self
                    .tools
                    .specs()
                    .into_iter()
                    .map(|spec| spec.name)
                    .collect(),
                tool_security: self.tools.diagnostics(),
                workspace_root: self
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.root().to_path_buf()),
                granted_workspace_roots: self
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.granted_roots().to_vec())
                    .unwrap_or_default(),
                prompt_sources,
                approval_audit: self.approval_audit.snapshot(),
            },
            crate::diagnostics::ExecutionSettings {
                event_capacity: self.event_capacity.get(),
                max_steps: self.max_steps.get(),
                compaction_trigger_messages: self
                    .compaction_policy
                    .as_ref()
                    .map(|policy| policy.trigger_messages().get()),
                reasoning_level: self.reasoning_level,
            },
        )
    }

    pub async fn session(&self, options: SessionOptions) -> Result<Session, Error> {
        if self.lifecycle.is_shutdown() {
            return Err(Error::RuntimeShutdown);
        }
        let mut history = options.history;
        if options.apply_system_prompt {
            if let SystemPrompt::Custom(prompt) = &self.system_prompt {
                history.insert(0, Message::System(prompt.clone()));
            }
        }
        Ok(Session::from_core(SessionCore::new(
            options.id,
            history,
            options.revision,
            options.compaction,
            options.prompt_cache_key,
            self.clone(),
        )))
    }
}

impl std::fmt::Debug for Rho {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Rho")
            .field("provider", &self.provider.identity())
            .field("tools", &self.tools)
            .field("system_prompt", &self.system_prompt)
            .field("event_capacity", &self.event_capacity)
            .field("max_steps", &self.max_steps)
            .field("workspace", &self.workspace)
            .field("workspace_policy", &self.workspace_policy)
            .field("approval_handler", &self.approval_handler)
            .field("compaction_policy", &self.compaction_policy)
            .field("reasoning_level", &self.reasoning_level)
            .finish()
    }
}
