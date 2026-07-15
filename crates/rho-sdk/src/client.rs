use std::{num::NonZeroUsize, path::PathBuf, sync::Arc};

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
    apply_system_prompt: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            id: SessionId::default(),
            history: Vec::new(),
            revision: crate::Revision::INITIAL,
            compaction: crate::CompactionState::default(),
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

    pub fn from_snapshot(snapshot: SessionSnapshot) -> Self {
        Self {
            id: snapshot.session_id().clone(),
            history: snapshot.history().to_vec(),
            revision: snapshot.revision(),
            compaction: snapshot.compaction().clone(),
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
    workspace_root: Option<PathBuf>,
    compactor: Option<Arc<dyn crate::Compactor>>,
    compaction_policy: Option<crate::CompactionPolicy>,
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

    /// Grants custom tools an explicit workspace root.
    pub fn workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(workspace_root.into());
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
            workspace_root: self.workspace_root,
            compactor: self.compactor,
            compaction_policy: self.compaction_policy,
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
    pub(crate) workspace_root: Option<PathBuf>,
    pub(crate) compactor: Option<Arc<dyn crate::Compactor>>,
    pub(crate) compaction_policy: Option<crate::CompactionPolicy>,
}

impl Rho {
    pub fn builder() -> RhoBuilder {
        RhoBuilder::default()
    }

    pub fn diagnostics(&self) -> crate::DiagnosticsSnapshot {
        let prompt_sources = match &self.system_prompt {
            SystemPrompt::None => Vec::new(),
            SystemPrompt::Custom(_) => vec![crate::diagnostics::PromptSource::custom()],
        };
        crate::DiagnosticsSnapshot::new(
            self.provider.identity(),
            self.tools
                .specs()
                .into_iter()
                .map(|spec| spec.name)
                .collect(),
            self.workspace_root.clone(),
            prompt_sources,
            self.event_capacity.get(),
            self.max_steps.get(),
            self.compaction_policy
                .as_ref()
                .map(|policy| policy.trigger_messages().get()),
        )
    }

    pub async fn session(&self, options: SessionOptions) -> Result<Session, Error> {
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
            .field("workspace_root", &self.workspace_root)
            .field("compaction_policy", &self.compaction_policy)
            .finish()
    }
}
