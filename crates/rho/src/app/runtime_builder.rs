use std::{num::NonZeroU64, sync::Arc};

use rho_sdk::{
    model::{ContentBlock, ModelRequest, ModelResponse},
    provider::ModelProvider,
    CompactionFuture, CompactionOutput, CompactionPolicy, CompactionRequest, Compactor, Error, Rho,
    SystemPrompt, Workspace, WorkspacePolicy,
};

use crate::{
    compaction::{
        build_summary_request_messages, partition_messages_for_compaction,
        replacement_history_from_summary, CompactionConfig,
    },
    config::Config,
    model::models_dev::cached_model_metadata,
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
}

pub(crate) fn build_runtime<P>(options: RuntimeBuildOptions<'_, P>) -> Result<Rho, Error>
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
    } = options;
    let (compactor, policy) = build_compaction(
        Arc::clone(&provider),
        tools,
        reasoning,
        compaction,
        context_window,
    );
    let mut builder = Rho::builder()
        .provider_shared(provider)
        .system_prompt(system_prompt)
        .workspace(workspace)
        .workspace_policy(workspace_policy)
        .reasoning_level(reasoning)
        .max_steps(super::sdk_config::run_step_limit())
        .compactor(compactor);
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
) -> (ModelCompactor, Option<CompactionPolicy>) {
    let policy = automatic_compaction_policy(&compaction, context_window);
    let compactor = ModelCompactor {
        provider,
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
