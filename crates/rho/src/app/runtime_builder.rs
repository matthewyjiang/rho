use std::{num::NonZeroU64, sync::Arc};

use rho_sdk::{
    model::{ContentBlock, ModelRequest, ModelResponse},
    provider::{ModelProvider, ModelRequestOptions},
    CompactionFuture, CompactionOutput, CompactionPolicy, CompactionRequest, Compactor, Error,
    ProviderRequestOutcome, ProviderRequestUsageContext, ProviderRequestUsageEvent,
    ProviderRequestUsageRecording, Rho, SystemPrompt, Workspace, WorkspacePolicy,
};

use {
    crate::compaction::{
        build_summary_request_messages, elide_tool_results, partition_messages_for_compaction,
        replacement_history_from_summary, strip_analysis, ActiveGoal, CompactionConfig,
    },
    crate::config::Config,
    crate::diagnostics::{CompactionTier, CompactionTierReport, RuntimeDiagnostics},
    crate::session::recall::RecallStore,
    rho_providers::model::models_dev::cached_model_metadata,
};

pub(crate) struct RuntimeBuildOptions<'a, P> {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) tools: &'a [Arc<dyn rho_sdk::tool::Tool>],
    pub(crate) workspace: Workspace,
    pub(crate) workspace_policy: P,
    pub(crate) approval_session: Option<rho_sdk::ApprovalSession>,
    pub(crate) system_prompt: SystemPrompt,
    pub(crate) reasoning: rho_sdk::ReasoningLevel,
    pub(crate) service_tier: Option<rho_sdk::model::ServiceTier>,
    pub(crate) compaction: CompactionConfig,
    pub(crate) context_window: Option<u64>,
    pub(crate) usage_purpose: &'static str,
    pub(crate) usage_parent_session_id: Option<rho_sdk::SessionId>,
    pub(crate) usage_recording: ProviderRequestUsageRecording,
    pub(crate) hook_host_labels: rho_sdk::hooks::HookHostLabels,
    /// Shared hook pipeline, or `None` when no hooks are configured.
    ///
    /// Borrowed rather than owned because the interactive host rebuilds its
    /// runtime on permission or provider changes and must reuse one pipeline.
    pub(crate) hooks: Option<&'a crate::hooks::HookPipeline>,
    /// Receives compaction tier reports for `/info` and `rho(action="compaction")`.
    pub(crate) diagnostics: RuntimeDiagnostics,
    /// From [`crate::tools::AppToolSet::recall_store`]; `None` disables elision.
    pub(crate) recall: Option<RecallStore>,
    /// From [`crate::tools::AppToolSet::active_goal`].
    pub(crate) active_goal: ActiveGoal,
}

pub(crate) fn build_runtime<P>(options: RuntimeBuildOptions<'_, P>) -> Result<Rho, Error>
where
    P: WorkspacePolicy + 'static,
{
    build_runtime_with_max_steps(options, None)
}

/// Builds a runtime with an optional per-run model-step override.
///
/// `None` keeps the configured SDK step limit. Automation uses `Some` for the
/// CLI's `--max-steps` option without changing interactive runtimes.
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
        approval_session,
        system_prompt,
        reasoning,
        service_tier,
        compaction,
        context_window,
        usage_purpose,
        usage_parent_session_id,
        usage_recording,
        hook_host_labels,
        hooks,
        diagnostics,
        recall,
        active_goal,
    } = options;
    let (compactor, policy) = build_compaction(CompactionSetup {
        provider: Arc::clone(&provider),
        tools,
        reasoning,
        compaction,
        context_window,
        usage_recording: usage_recording.clone(),
        diagnostics,
        recall,
        active_goal,
    });
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
        .hook_host_labels(hook_host_labels)
        .compactor(compactor);
    if let Some(service_tier) = service_tier {
        builder = builder.service_tier(service_tier);
    }
    if let Some(parent_session_id) = usage_parent_session_id {
        // A delegated run reports the same parentage to accounting and to hooks,
        // so a hook can attribute nested subagent work to the session that asked
        // for it.
        builder = builder
            .hook_delegation(rho_sdk::hooks::HookDelegation::new(
                parent_session_id.clone(),
            ))
            .usage_parent_session_id(parent_session_id);
    }
    if let Some(session) = approval_session {
        builder = builder.approval_session(session);
    }
    if let Some(policy) = policy {
        builder = builder.compaction_policy(policy);
    }
    for tool in tools {
        builder = builder.tool_shared(tool.clone());
    }
    if let Some(hooks) = hooks {
        builder = hooks.attach(builder);
    }
    builder.build()
}

/// Inputs for the host compactor and its automatic policy. Every runtime build
/// and live refresh constructs the compactor from this one shape.
pub(crate) struct CompactionSetup<'a> {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) tools: &'a [Arc<dyn rho_sdk::tool::Tool>],
    pub(crate) reasoning: rho_sdk::ReasoningLevel,
    pub(crate) compaction: CompactionConfig,
    pub(crate) context_window: Option<u64>,
    pub(crate) usage_recording: ProviderRequestUsageRecording,
    /// Receives which compaction tier ran.
    pub(crate) diagnostics: RuntimeDiagnostics,
    /// Where elided originals are saved. `None` turns elision off, because the
    /// agent could not recall them.
    pub(crate) recall: Option<RecallStore>,
    /// Active `/goal` kept verbatim by text-summary compaction.
    pub(crate) active_goal: ActiveGoal,
}

pub(crate) fn build_compaction(
    setup: CompactionSetup<'_>,
) -> (ModelCompactor, Option<CompactionPolicy>) {
    let CompactionSetup {
        provider,
        tools,
        reasoning,
        compaction,
        context_window,
        usage_recording,
        diagnostics,
        recall,
        active_goal,
    } = setup;
    let policy = automatic_compaction_policy(&compaction, context_window);
    let compactor = ModelCompactor {
        provider,
        usage_recording,
        tool_specs: tools.iter().map(|tool| tool.spec()).collect(),
        reasoning,
        config: compaction,
        context_window,
        diagnostics,
        recall,
        active_goal,
    };
    (compactor, policy)
}

/// Rebuilds the live session's compactor and automatic policy from the same
/// inputs `build_compaction` uses at runtime construction.
pub(crate) fn refresh_session_compaction(
    session: &rho_sdk::Session,
    setup: CompactionSetup<'_>,
) -> Result<(), Error> {
    let (compactor, policy) = build_compaction(setup);
    session.set_compaction(Some(Arc::new(compactor)), policy)
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
    usage_recording: ProviderRequestUsageRecording,
    tool_specs: Vec<rho_sdk::model::ToolSpec>,
    reasoning: rho_sdk::ReasoningLevel,
    config: CompactionConfig,
    context_window: Option<u64>,
    diagnostics: RuntimeDiagnostics,
    recall: Option<RecallStore>,
    active_goal: ActiveGoal,
}

impl Compactor for ModelCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            let mut usage_context =
                ProviderRequestUsageContext::for_purpose(self.provider.identity(), "compaction");
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
            let mut next_attempt_index = 1usize;

            // Tier 1: elide old tool results. Commit without a model request
            // when that alone reaches the target; otherwise later tiers see the
            // elided history.
            let context = request.context_estimate().unwrap_or_else(|| {
                rho_sdk::ContextEstimate::from_estimated_tokens(
                    rho_sdk::model::context::estimate_context_tokens(
                        request.messages(),
                        &self.tool_specs,
                    ),
                )
            });
            let target_tokens = self.config.target_tokens_for_context(
                self.context_window,
                request.trigger(),
                context,
            );
            let (elided, elided_tool_results) = match self.elide(&request, target_tokens) {
                Some(elision) => (Some(elision.messages), elision.originals.len()),
                None => (None, 0),
            };
            let report = |tier| {
                self.diagnostics
                    .record_compaction_tier(CompactionTierReport {
                        tier,
                        elided_tool_results,
                    })
            };
            if let Some(elided) = &elided {
                let tokens =
                    rho_sdk::model::context::estimate_context_tokens(elided, &self.tool_specs);
                if tokens <= target_tokens {
                    report(CompactionTier::Elision);
                    return CompactionOutput::new(elided.clone());
                }
            }
            let messages = elided.as_deref().unwrap_or(request.messages());

            match self
                .try_native_compaction(
                    messages,
                    request.service_tier(),
                    cancellation.clone(),
                    usage_context.clone(),
                    &mut next_attempt_index,
                )
                .await
            {
                NativeCompactionResult::Success(output) => {
                    report(CompactionTier::Native);
                    return Ok(output);
                }
                NativeCompactionResult::Cancelled => return Err(Error::Cancelled),
                // Explicit fallback to portable text-summary compaction.
                NativeCompactionResult::Unavailable | NativeCompactionResult::Failed => {}
            }

            let Some(partition) =
                partition_messages_for_compaction(messages, &self.tool_specs, target_tokens)
            else {
                report(match elided {
                    Some(_) => CompactionTier::Elision,
                    None => CompactionTier::Unchanged,
                });
                return CompactionOutput::new(messages.to_vec());
            };
            let summary_messages = build_summary_request_messages(&partition);
            let model_request = ModelRequest {
                messages: &summary_messages,
                tools: &[],
                cancellation: cancellation.clone(),
                reasoning_level: self.reasoning,
                // CompactionRequest has no canonical prompt cache key; keep None.
                prompt_cache_key: None,
            };
            let (response, usage) = match crate::usage::send_recorded_from_attempt(
                self.provider.as_ref(),
                model_request,
                usage_context,
                self.usage_recording.clone(),
                next_attempt_index,
            )
            .await
            {
                Ok(result) => result,
                Err(_) if cancellation.is_cancelled() => return Err(Error::Cancelled),
                Err(error) => return Err(error.into()),
            };
            let ModelResponse::Assistant(blocks) = response;
            let summary = strip_analysis(
                &blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            );
            if summary.is_empty() {
                return Err(Error::InvalidHostResponse {
                    message: "compaction model returned no summary text".into(),
                });
            }
            report(CompactionTier::TextSummary);
            CompactionOutput::with_usage(
                replacement_history_from_summary(
                    partition,
                    request.trigger(),
                    &self.active_goal,
                    &summary,
                ),
                usage,
            )
        })
    }

    fn cancellation_mode(&self) -> rho_sdk::CompactorCancellationMode {
        rho_sdk::CompactorCancellationMode::Cooperative
    }
}

#[derive(Debug)]
enum NativeCompactionResult {
    Success(CompactionOutput),
    Failed,
    Unavailable,
    Cancelled,
}

impl ModelCompactor {
    /// Elides only when the agent can recall, and only after the originals are
    /// saved. A failed save skips elision rather than stranding a stub.
    fn elide(
        &self,
        request: &CompactionRequest,
        target_tokens: u64,
    ) -> Option<crate::compaction::Elision> {
        let dir = self.recall.as_ref()?.dir()?;
        let elision = elide_tool_results(request.messages(), &self.tool_specs, target_tokens)?;
        match crate::session::recall::save(&dir, &elision.originals) {
            Ok(()) => Some(elision),
            Err(error) => {
                tracing::warn!(%error, "could not save elided tool results; skipping elision");
                None
            }
        }
    }

    async fn try_native_compaction(
        &self,
        messages: &[rho_sdk::model::Message],
        service_tier: Option<rho_sdk::model::ServiceTier>,
        cancellation: rho_sdk::CancellationToken,
        usage_context: ProviderRequestUsageContext,
        next_attempt_index: &mut usize,
    ) -> NativeCompactionResult {
        let model_request = ModelRequest {
            messages,
            // Native compact requests do not advertise tools.
            tools: &[],
            cancellation: cancellation.clone(),
            reasoning_level: self.reasoning,
            // CompactionRequest has no canonical prompt cache key; keep None.
            prompt_cache_key: None,
        };
        let options = match service_tier {
            Some(tier) => ModelRequestOptions::default().with_service_tier(tier),
            None => ModelRequestOptions::default(),
        };
        let Some(future) = self
            .provider
            .native_compact_with_options(model_request, options)
        else {
            return NativeCompactionResult::Unavailable;
        };
        let response = future.await;
        let (result, failed_attempts) = response.into_parts();
        for attempt in failed_attempts {
            self.usage_recording
                .record(ProviderRequestUsageEvent::observed(
                    usage_context
                        .clone()
                        .with_attempt_index(*next_attempt_index),
                    attempt.usage,
                    ProviderRequestOutcome::Failed(attempt.kind),
                ))
                .await;
            *next_attempt_index += 1;
        }
        let outcome = match &result {
            Ok(_) => ProviderRequestOutcome::Completed,
            Err(_) if cancellation.is_cancelled() => ProviderRequestOutcome::Cancelled,
            Err(error) => ProviderRequestOutcome::Failed(error.kind()),
        };
        let usage = result
            .as_ref()
            .map(|output| output.usage().clone())
            .unwrap_or_default();
        self.usage_recording
            .record(ProviderRequestUsageEvent::observed(
                usage_context.with_attempt_index(*next_attempt_index),
                usage,
                outcome,
            ))
            .await;
        *next_attempt_index += 1;
        match result {
            Ok(output) => NativeCompactionResult::Success(output),
            Err(_) if cancellation.is_cancelled() => NativeCompactionResult::Cancelled,
            Err(_) => NativeCompactionResult::Failed,
        }
    }
}

#[cfg(test)]
#[path = "runtime_builder_tests.rs"]
mod tests;
