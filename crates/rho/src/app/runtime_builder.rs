use std::{num::NonZeroU64, sync::Arc};

use rho_sdk::{
    model::{ContentBlock, ModelRequest, ModelResponse},
    provider::ModelProvider,
    CompactionFuture, CompactionOutput, CompactionPolicy, CompactionRequest, Compactor, Error, Rho,
    SystemPrompt, Workspace, WorkspacePolicy,
};

use {
    crate::compaction::{
        build_summary_request_messages, partition_messages_for_compaction,
        replacement_history_from_summary, CompactionConfig,
    },
    crate::config::Config,
    rho_providers::model::models_dev::cached_model_metadata,
};

pub(crate) struct RuntimeBuildOptions<'a, P> {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) tools: &'a [Arc<dyn rho_sdk::tool::Tool>],
    pub(crate) workspace: Workspace,
    pub(crate) workspace_policy: P,
    pub(crate) approval_handler: Option<Arc<dyn rho_sdk::ApprovalHandler>>,
    pub(crate) system_prompt: SystemPrompt,
    pub(crate) reasoning: rho_sdk::ReasoningLevel,
    pub(crate) compaction: CompactionConfig,
    pub(crate) context_window: Option<u64>,
    pub(crate) usage_purpose: &'static str,
    pub(crate) usage_parent_session_id: Option<rho_sdk::SessionId>,
    pub(crate) usage_recording: rho_sdk::ProviderRequestUsageRecording,
}

pub(crate) fn build_runtime<P>(options: RuntimeBuildOptions<'_, P>) -> Result<Rho, Error>
where
    P: WorkspacePolicy + 'static,
{
    build_runtime_with_max_steps(options, None)
}

pub(crate) fn build_runtime_with_max_steps<P>(
    options: RuntimeBuildOptions<'_, P>,
    max_steps: Option<std::num::NonZeroUsize>,
) -> Result<Rho, Error>
where
    P: WorkspacePolicy + 'static,
{
    let RuntimeBuildOptions {
        provider,
        tools,
        workspace,
        workspace_policy,
        approval_handler,
        system_prompt,
        reasoning,
        compaction,
        context_window,
        usage_purpose,
        usage_parent_session_id,
        usage_recording,
    } = options;
    let (compactor, policy) = build_compaction(
        Arc::clone(&provider),
        tools,
        reasoning,
        compaction,
        context_window,
        usage_recording.clone(),
    );
    let mut builder = Rho::builder()
        .provider_shared(provider)
        .system_prompt(system_prompt)
        .workspace(workspace)
        .workspace_policy(workspace_policy)
        .reasoning_level(reasoning)
        .max_steps(max_steps.unwrap_or_else(super::sdk_config::run_step_limit))
        .max_parallel_tools(super::sdk_config::parallel_tool_limit())
        .usage_purpose(usage_purpose)
        .usage_recording(usage_recording)
        .compactor(compactor);
    if let Some(parent_session_id) = usage_parent_session_id {
        builder = builder.usage_parent_session_id(parent_session_id);
    }
    if let Some(handler) = approval_handler {
        builder = builder.approval_handler_shared(handler);
    }
    if let Some(policy) = policy {
        builder = builder.compaction_policy(policy);
    }
    for tool in tools {
        builder = builder.tool_shared(tool.clone());
    }
    builder.build()
}

pub(crate) fn build_compaction(
    provider: Arc<dyn ModelProvider>,
    tools: &[Arc<dyn rho_sdk::tool::Tool>],
    reasoning: rho_sdk::ReasoningLevel,
    compaction: CompactionConfig,
    context_window: Option<u64>,
    usage_recording: rho_sdk::ProviderRequestUsageRecording,
) -> (ModelCompactor, Option<CompactionPolicy>) {
    let policy = automatic_compaction_policy(&compaction, context_window);
    let compactor = ModelCompactor {
        provider,
        usage_recording,
        tool_specs: tools.iter().map(|tool| tool.spec()).collect(),
        reasoning,
        config: compaction,
        context_window,
    };
    (compactor, policy)
}

pub(crate) fn automatic_compaction_policy(
    compaction: &CompactionConfig,
    context_window: Option<u64>,
) -> Option<CompactionPolicy> {
    context_window
        .and_then(|window| compaction.threshold_tokens(window))
        .and_then(NonZeroU64::new)
        .map(CompactionPolicy::at_context_tokens)
}

pub(crate) fn configured_context_window(config: &Config) -> Option<u64> {
    cached_model_metadata(&config.provider, &config.model)
        .and_then(|metadata| metadata.display_context_window())
}

pub(crate) struct ModelCompactor {
    provider: Arc<dyn ModelProvider>,
    usage_recording: rho_sdk::ProviderRequestUsageRecording,
    tool_specs: Vec<rho_sdk::model::ToolSpec>,
    reasoning: rho_sdk::ReasoningLevel,
    config: CompactionConfig,
    context_window: Option<u64>,
}

impl Compactor for ModelCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            let mut usage_context = rho_sdk::ProviderRequestUsageContext::for_purpose(
                self.provider.identity(),
                "compaction",
            );
            if let Some(session_id) = request.session_id() {
                usage_context = usage_context.with_session_id(session_id.clone());
            }
            if let Some(parent_session_id) = request.parent_session_id() {
                usage_context = usage_context.with_parent_session_id(parent_session_id.clone());
            }
            if let Some(run_id) = request.run_id() {
                usage_context = usage_context.with_run_id(run_id.clone());
            }
            if let Some(step_index) = request.step_index() {
                usage_context = usage_context.with_step_index(step_index);
            }
            if let Some(workspace_path) = request.workspace_path() {
                usage_context = usage_context.with_workspace_path(workspace_path.to_path_buf());
            }
            let cancellation = request.cancellation().clone();
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
                cancellation: cancellation.clone(),
                reasoning_level: self.reasoning,
                prompt_cache_key: None,
            };
            let (response, usage) = match crate::usage::send_recorded(
                self.provider.as_ref(),
                model_request,
                usage_context,
                self.usage_recording.clone(),
            )
            .await
            {
                Ok(result) => result,
                Err(_) if cancellation.is_cancelled() => return Err(Error::Cancelled),
                Err(error) => return Err(error.into()),
            };
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
            CompactionOutput::with_usage(
                replacement_history_from_summary(partition, summary),
                usage,
            )
        })
    }

    fn cancellation_mode(&self) -> rho_sdk::CompactorCancellationMode {
        rho_sdk::CompactorCancellationMode::Cooperative
    }
}
