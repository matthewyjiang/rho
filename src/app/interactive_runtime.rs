use std::{path::PathBuf, sync::Arc};

use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    provider::ModelProvider,
    CapabilityRequest, CompactionFuture, CompactionOutput, CompactionRequest, Compactor, Error,
    HostInputId, HostInputResponse, PolicyDecision, Rho, Run, RunEvent, RunOutcome, Session,
    SessionId, SessionOptions, SystemPrompt, UserInput, Workspace, WorkspacePolicy,
};

use crate::{
    compaction::{
        build_summary_request_messages, partition_messages_for_compaction,
        replacement_history_from_summary, CompactionConfig,
    },
    config::Config,
    credentials::OsCredentialStore,
    diagnostics::RuntimeDiagnostics,
    model::models_dev::cached_model_metadata,
    prompt,
    providers::{build_sdk_provider_with_source, UnavailableProvider},
    session::Session as StoredSession,
    tools::sdk_registry::AutomationToolSet,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InteractiveState {
    #[default]
    Idle,
    Running,
    WaitingForHostInput,
    Cancelling,
    Compacting,
    Completed,
    Failed,
}

pub(crate) struct InteractiveRuntimeOptions<'a> {
    pub(crate) config: &'a Config,
    pub(crate) cwd: PathBuf,
    pub(crate) no_system_prompt: bool,
    pub(crate) no_tools: bool,
    pub(crate) questionnaire_enabled: bool,
    pub(crate) history: Vec<Message>,
    pub(crate) session_id: Option<String>,
    pub(crate) storage: Option<StoredSession>,
    pub(crate) diagnostics: RuntimeDiagnostics,
    pub(crate) unavailable_error: Option<crate::model::ModelError>,
}

pub(crate) struct InteractiveRuntime {
    runtime: Rho,
    session: Session,
    active_run: Option<Run>,
    state: InteractiveState,
    provider: Arc<dyn ModelProvider>,
    tools: AutomationToolSet,
    workspace: Workspace,
    system_prompt: SystemPrompt,
    reasoning: rho_sdk::ReasoningLevel,
    compaction: CompactionConfig,
    context_window: Option<u64>,
    storage: Option<StoredSession>,
    pending_model_user: Option<Message>,
    pending_display_user: Option<Message>,
    pending_session_id: Option<SessionId>,
}

impl InteractiveRuntime {
    pub(crate) async fn new(options: InteractiveRuntimeOptions<'_>) -> anyhow::Result<Self> {
        let InteractiveRuntimeOptions {
            config,
            cwd,
            no_system_prompt,
            no_tools,
            questionnaire_enabled,
            history,
            session_id,
            storage,
            diagnostics,
            unavailable_error,
        } = options;
        let sdk_options = super::sdk_config::SdkBootstrapOptions::from_config(config, &cwd)?;
        let provider: Arc<dyn ModelProvider> = match unavailable_error {
            Some(error) => Arc::new(UnavailableProvider::new(error)),
            None => {
                let credentials =
                    crate::auth::provider_credentials::ApplicationCredentialSource::new(Arc::new(
                        OsCredentialStore,
                    ));
                build_sdk_provider_with_source(sdk_options.provider.clone(), &credentials)?
            }
        };
        let tools = if no_tools {
            AutomationToolSet::disabled()
        } else {
            AutomationToolSet::interactive(config, diagnostics.clone(), questionnaire_enabled)
        };
        let specs = tools.specs();
        let system_prompt = if no_system_prompt {
            diagnostics.update_prompt_sources(Vec::new());
            SystemPrompt::None
        } else {
            let built = prompt::system_prompt(&specs, &cwd);
            diagnostics.update_prompt_sources(built.sources);
            SystemPrompt::Custom(built.text)
        };
        diagnostics.update_tools(&specs);
        let workspace = Workspace::new(&sdk_options.workspace.root)?;
        let context_window = cached_model_metadata(&config.provider, &config.model)
            .and_then(|metadata| metadata.display_context_window());
        let compaction = sdk_options.runtime.compaction.clone();
        diagnostics.update_compaction_config(&compaction);
        let runtime = build_runtime(
            Arc::clone(&provider),
            &tools,
            workspace.clone(),
            system_prompt.clone(),
            sdk_options.runtime.reasoning,
            compaction.clone(),
            context_window,
        )?;
        let cache_key = session_id.as_deref().map(prompt_cache_key);
        let resumed_snapshot = storage
            .as_ref()
            .map(|storage| {
                storage.snapshot_for_resume(
                    provider.identity(),
                    cache_key
                        .clone()
                        .unwrap_or_else(|| prompt_cache_key(storage.id())),
                )
            })
            .transpose()?;
        let options = if let Some(snapshot) = resumed_snapshot {
            report_resume_omissions(&snapshot, &provider.identity());
            SessionOptions::from_snapshot(snapshot)
        } else {
            let mut options = SessionOptions::new().history(history);
            if let Some(id) = session_id.as_deref() {
                options = options
                    .id(SessionId::from_string(id)?)
                    .prompt_cache_key(cache_key.unwrap_or_else(|| prompt_cache_key(id)));
            }
            options
        };
        let session = runtime.session(options).await?;
        Ok(Self {
            runtime,
            session,
            active_run: None,
            state: InteractiveState::Idle,
            provider,
            tools,
            workspace,
            system_prompt,
            reasoning: sdk_options.runtime.reasoning,
            compaction,
            context_window,
            storage,
            pending_model_user: None,
            pending_display_user: None,
            pending_session_id: None,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn state(&self) -> InteractiveState {
        self.state
    }

    pub(crate) fn history(&self) -> Vec<Message> {
        self.session.history()
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        self.pending_session_id
            .as_ref()
            .unwrap_or_else(|| self.session.id())
    }

    pub(crate) fn set_context_window(&mut self, context_window: Option<u64>) {
        self.context_window = context_window;
    }

    pub(crate) fn attach_storage(&mut self, storage: StoredSession) {
        self.storage = Some(storage);
    }

    pub(crate) async fn start(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
    ) -> Result<(), Error> {
        if self.state != InteractiveState::Idle {
            return Err(Error::SessionBusy);
        }
        if let Some(id) = self.pending_session_id.take() {
            self.rebuild_session(Vec::new(), Some(id.to_string()))
                .await
                .map_err(|error| Error::Persistence {
                    message: error.to_string(),
                })?;
        }
        let model_user = Message::User(input.blocks().to_vec());
        let run = self.session.start(input).await?;
        self.pending_model_user = Some(model_user);
        self.pending_display_user = display_user;
        self.active_run = Some(run);
        self.state = InteractiveState::Running;
        Ok(())
    }

    pub(crate) async fn next_event(&mut self) -> Option<RunEvent> {
        let event = self.active_run.as_mut()?.next_event().await;
        if let Some(event) = &event {
            self.observe_event(event);
        }
        event
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(run) = &self.active_run {
            run.cancel();
            self.state = InteractiveState::Cancelling;
        }
    }

    pub(crate) async fn steer(&self, input: UserInput) -> Result<(), Error> {
        self.active_run
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts steering input".into(),
            })?
            .steer(input)
            .await
    }

    pub(crate) async fn respond(
        &mut self,
        request_id: HostInputId,
        response: HostInputResponse,
    ) -> Result<(), Error> {
        self.active_run
            .as_ref()
            .ok_or(Error::InvalidHostResponse {
                message: "no active run accepts host input".into(),
            })?
            .respond(request_id, response)
            .await?;
        self.state = InteractiveState::Running;
        Ok(())
    }

    pub(crate) async fn finish_run(&mut self) -> anyhow::Result<RunOutcome> {
        let mut run = self
            .active_run
            .take()
            .ok_or_else(|| anyhow::anyhow!("no active run"))?;
        let outcome = run.outcome().await;
        self.sync_storage()?;
        self.pending_model_user = None;
        self.pending_display_user = None;
        self.state = InteractiveState::Idle;
        Ok(outcome?)
    }

    pub(crate) async fn compact(&mut self) -> anyhow::Result<bool> {
        if self.state != InteractiveState::Idle {
            anyhow::bail!("session is busy");
        }
        self.state = InteractiveState::Compacting;
        let result = self.session.compact().await;
        self.state = InteractiveState::Idle;
        let outcome = result?;
        self.sync_storage_replace()?;
        Ok(outcome.current_messages() < outcome.previous_messages())
    }

    pub(crate) fn reset(&mut self) -> anyhow::Result<()> {
        if self.active_run.is_some() {
            anyhow::bail!("cannot reset while a run is active");
        }
        self.session.reset()?;
        self.storage = None;
        self.pending_session_id = Some(SessionId::new());
        self.state = InteractiveState::Idle;
        Ok(())
    }

    pub(crate) async fn resume(
        &mut self,
        storage: StoredSession,
        _history: Vec<Message>,
    ) -> anyhow::Result<()> {
        if self.active_run.is_some() {
            anyhow::bail!("cannot resume while a run is active");
        }
        let id = storage.id().to_string();
        self.storage = Some(storage);
        self.rebuild_session(Vec::new(), Some(id)).await
    }

    pub(crate) fn replace_provider(
        &mut self,
        provider: Arc<dyn ModelProvider>,
        reasoning: rho_sdk::ReasoningLevel,
    ) -> Result<rho_sdk::model::handoff::HandoffReport, Error> {
        let report = self.session.replace_provider(Arc::clone(&provider))?;
        self.session.set_reasoning_level(reasoning)?;
        self.provider = provider;
        self.reasoning = reasoning;
        Ok(report)
    }

    pub(crate) fn append_user_context_with_display(
        &mut self,
        model: String,
        display: String,
    ) -> anyhow::Result<()> {
        let message = Message::user_text(model);
        self.session.append_message(message.clone())?;
        if let Some(storage) = &self.storage {
            storage.save_snapshot(&self.session.snapshot(), &[Message::user_text(display)])?;
        }
        Ok(())
    }

    pub(crate) fn load_skill(
        &mut self,
        skill: &crate::skills::Skill,
        max_bytes: usize,
    ) -> anyhow::Result<()> {
        let content = crate::tool::truncate(skill.contents.clone(), max_bytes);
        let message = Message::user_text(format!(
            "Loaded skill `{}` from {}:\n\n{}",
            skill.name, skill.source, content
        ));
        self.session.append_message(message.clone())?;
        if let Some(storage) = &self.storage {
            storage.save_snapshot(&self.session.snapshot(), std::slice::from_ref(&message))?;
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&mut self) {
        if self.active_run.is_some() {
            self.cancel();
            let _ = self.finish_run().await;
        }
        self.runtime.shutdown();
        self.tools.shutdown().await;
    }

    fn observe_event(&mut self, event: &RunEvent) {
        self.state = state_after_event(self.state, event);
    }

    async fn rebuild_session(
        &mut self,
        history: Vec<Message>,
        id: Option<String>,
    ) -> anyhow::Result<()> {
        self.runtime.shutdown();
        self.runtime = build_runtime(
            Arc::clone(&self.provider),
            &self.tools,
            self.workspace.clone(),
            self.system_prompt.clone(),
            self.reasoning,
            self.compaction.clone(),
            self.context_window,
        )?;
        let options = if let Some(storage) = &self.storage {
            let snapshot = storage.snapshot_for_resume(
                self.provider.identity(),
                id.as_deref()
                    .map(prompt_cache_key)
                    .unwrap_or_else(|| prompt_cache_key(storage.id())),
            )?;
            report_resume_omissions(&snapshot, &self.provider.identity());
            SessionOptions::from_snapshot(snapshot)
        } else {
            let mut options = SessionOptions::new().history(history);
            if let Some(id) = id.as_deref() {
                options = options
                    .id(SessionId::from_string(id)?)
                    .prompt_cache_key(prompt_cache_key(id));
            }
            options
        };
        self.session = self.runtime.session(options).await?;
        self.pending_session_id = None;
        self.state = InteractiveState::Idle;
        Ok(())
    }

    fn sync_storage(&mut self) -> anyhow::Result<()> {
        let history = self.session.history();
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        let display_start = self
            .pending_model_user
            .as_ref()
            .and_then(|user| history.iter().rposition(|message| message == user))
            .unwrap_or(history.len());
        let mut display_tail = history[display_start..].to_vec();
        if let (Some(display), Some(first)) = (&self.pending_display_user, display_tail.first_mut())
        {
            *first = display.clone();
        }
        storage.save_snapshot(&self.session.snapshot(), &display_tail)?;
        Ok(())
    }

    fn sync_storage_replace(&mut self) -> anyhow::Result<()> {
        if let Some(storage) = &self.storage {
            storage.save_snapshot(&self.session.snapshot(), &[])?;
        }
        Ok(())
    }
}

fn prompt_cache_key(id: &str) -> String {
    crate::providers::openai::prompt_cache_key_from_session_id(id)
        .unwrap_or_else(|| format!("rho:{id}"))
}

fn report_resume_omissions(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) {
    let report = snapshot.provider_context_omissions(target);
    if report.has_omissions() {
        eprintln!(
            "warning: omitted {} incompatible provider-native context block(s) while resuming session (kinds: {})",
            report.omitted_provider_context,
            report.omitted_kinds.join(", ")
        );
    }
}

fn state_after_event(current: InteractiveState, event: &RunEvent) -> InteractiveState {
    match event {
        RunEvent::HostInputRequested { .. } => InteractiveState::WaitingForHostInput,
        RunEvent::Completed { .. } | RunEvent::Cancelled { .. } => InteractiveState::Completed,
        RunEvent::Failed { .. } => InteractiveState::Failed,
        _ if current == InteractiveState::Cancelling => InteractiveState::Cancelling,
        _ => InteractiveState::Running,
    }
}

fn build_runtime(
    provider: Arc<dyn ModelProvider>,
    tools: &AutomationToolSet,
    workspace: Workspace,
    system_prompt: SystemPrompt,
    reasoning: rho_sdk::ReasoningLevel,
    compaction: CompactionConfig,
    context_window: Option<u64>,
) -> Result<Rho, Error> {
    let compactor = ModelCompactor {
        provider: Arc::clone(&provider),
        tool_specs: tools.specs(),
        reasoning,
        config: compaction,
        context_window,
    };
    let mut builder = Rho::builder()
        .provider_shared(provider)
        .system_prompt(system_prompt)
        .workspace(workspace)
        .workspace_policy(InteractiveWorkspacePolicy)
        .reasoning_level(reasoning)
        .compactor(compactor);
    for tool in tools.tools() {
        builder = builder.tool_shared(tool.clone());
    }
    builder.build()
}

struct InteractiveWorkspacePolicy;

impl WorkspacePolicy for InteractiveWorkspacePolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

struct ModelCompactor {
    provider: Arc<dyn ModelProvider>,
    tool_specs: Vec<rho_sdk::model::ToolSpec>,
    reasoning: rho_sdk::ReasoningLevel,
    config: CompactionConfig,
    context_window: Option<u64>,
}

impl Compactor for ModelCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            let messages = request.messages().to_vec();
            let target_tokens = self
                .context_window
                .map(|window| self.config.target_tokens(window))
                .unwrap_or(u64::MAX / 2);
            let Some(partition) =
                partition_messages_for_compaction(&messages, &self.tool_specs, target_tokens)
            else {
                return CompactionOutput::new(messages);
            };
            let summary_messages = build_summary_request_messages(&partition.compacted_messages);
            let model_request = ModelRequest {
                messages: &summary_messages,
                tools: &[],
                cancellation: request.cancellation().clone(),
                reasoning_level: self.reasoning,
                prompt_cache_key: None,
            };
            let response = self.provider.send_turn(model_request).await?;
            let ModelResponse::Assistant(blocks) = response;
            let summary = blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if summary.trim().is_empty() {
                return Err(Error::InvalidHostResponse {
                    message: "compaction model returned no summary text".into(),
                });
            }
            CompactionOutput::new(replacement_history_from_summary(partition, summary))
        })
    }
}

#[cfg(test)]
#[path = "interactive_runtime_tests.rs"]
mod tests;
