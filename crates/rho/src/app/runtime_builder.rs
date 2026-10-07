use std::{num::NonZeroU64, sync::Arc};

use rho_sdk::{
    provider::ModelProvider, CompactionPolicy, Error, ProviderRequestUsageRecording, Rho,
    SystemPrompt, Workspace, WorkspacePolicy,
};

pub(crate) use super::model_compactor::ModelCompactor;
use {
    super::model_compactor::Summarizer, crate::compaction::CompactionConfig, crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics, crate::session::recall::RecallStore,
    rho_providers::model::models_dev::cached_model_metadata,
};

pub(crate) struct RuntimeBuildOptions<'a, P> {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) tools: &'a crate::tools::sdk_registry::AppToolSet,
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
    /// From [`crate::tools::sdk_registry::AppToolSet::recall_store`]; `None` disables elision.
    pub(crate) recall: Option<RecallStore>,
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
    } = options;
    let (compactor, policy) = build_compaction(CompactionSetup {
        provider: Arc::clone(&provider),
        tool_specs: tools.specs(),
        reasoning,
        compaction,
        context_window,
        usage_recording: usage_recording.clone(),
        diagnostics,
        recall,
        todo: Some(tools.todo_state()),
    });
    let mut builder = Rho::builder()
        .provider_shared(provider)
        .request_context(tools.todo_state())
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
    builder = builder.tool_visibility_shared(tools.tool_visibility());
    if let Some(session) = approval_session {
        builder = builder.approval_session(session);
    }
    if let Some(policy) = policy {
        builder = builder.compaction_policy(policy);
    }
    for tool in tools.tools() {
        builder = builder.tool_shared(tool.clone());
    }
    if let Some(hooks) = hooks {
        builder = hooks.attach(builder);
    }
    let runtime = builder.build()?;
    tools.todo_state().set_context_window(context_window);
    Ok(runtime)
}

/// Inputs for the host compactor and its automatic policy. Every runtime build
/// and live refresh constructs the compactor from this one shape.
pub(crate) struct CompactionSetup {
    pub(crate) provider: Arc<dyn ModelProvider>,
    /// Advertised schemas used only when a compaction request supplies none.
    pub(crate) tool_specs: Vec<rho_sdk::model::ToolSpec>,
    pub(crate) reasoning: rho_sdk::ReasoningLevel,
    pub(crate) compaction: CompactionConfig,
    pub(crate) context_window: Option<u64>,
    pub(crate) usage_recording: ProviderRequestUsageRecording,
    /// Receives which compaction tier ran.
    pub(crate) diagnostics: RuntimeDiagnostics,
    /// Where elided originals are saved. `None` turns elision off, because the
    /// agent could not recall them.
    pub(crate) recall: Option<RecallStore>,
    /// Host-owned exact checklist, shared with nested tool execution.
    pub(crate) todo: Option<crate::tools::todo::TodoState>,
}

pub(crate) fn build_compaction(
    setup: CompactionSetup,
) -> (ModelCompactor, Option<CompactionPolicy>) {
    let CompactionSetup {
        provider,
        tool_specs,
        reasoning,
        compaction,
        context_window,
        usage_recording,
        diagnostics,
        recall,
        todo,
    } = setup;
    let policy = automatic_compaction_policy(&compaction, context_window);
    let compactor = ModelCompactor {
        provider,
        usage_recording,
        tool_specs,
        reasoning,
        summarizer: compaction.summarizer.clone().map(Summarizer::new),
        config: compaction,
        context_window,
        diagnostics,
        recall,
        todo,
    };
    (compactor, policy)
}

/// Rebuilds the live session's compactor and automatic policy from the same
/// inputs `build_compaction` uses at runtime construction.
pub(crate) fn refresh_session_compaction(
    session: &rho_sdk::Session,
    setup: CompactionSetup,
) -> Result<(), Error> {
    let todo = setup.todo.clone();
    let context_window = setup.context_window;
    let (compactor, policy) = build_compaction(setup);
    session.set_compaction(Some(Arc::new(compactor)), policy)?;
    if let Some(todo) = todo {
        todo.set_context_window(context_window);
    }
    Ok(())
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

#[cfg(test)]
#[path = "runtime_builder_exposure_tests.rs"]
mod exposure_tests;
