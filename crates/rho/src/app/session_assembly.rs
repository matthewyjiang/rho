//! Shared startup path for non-interactive Rho sessions.
//!
//! Automation runs and ACP sessions both need the same assembly: provider,
//! workspace, tools and prompt, approval wiring, hooks, runtime, and session.
//! They differ only in the tool and prompt deltas, the approval recipe, and the
//! session options, so those arrive as call-site policy.

use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_providers::{
    auth::provider_credentials::ApplicationCredentialSource, providers::build_automation_provider,
};
use rho_sdk::{
    provider::ModelProvider, ApprovalRequestReceiver, ApprovalSession, Session, SessionOptions,
};

use super::{
    agent_binding::BoundAgent,
    policy::AppPolicy,
    runtime_builder::{
        build_runtime_with_max_steps, configured_context_window, RuntimeBuildOptions,
    },
    sdk_config::SdkBootstrapOptions,
    tools_prompt::{
        assemble_tools_and_prompt, McpSamplingSupport, ToolsAndPrompt, ToolsAndPromptOptions,
    },
};
use crate::{
    config::Config,
    credential_store::AppCredentialStore,
    diagnostics::RuntimeDiagnostics,
    permission::SessionWriteLog,
    tools::{agent::BackgroundSubagents, sdk_registry::AppToolSet},
};

/// Everything a caller must supply to assemble a session, including the three
/// policy hooks that differ between callers.
pub(super) struct SessionAssemblyOptions<'a, ExtendTools, Approval, Options> {
    pub config: &'a Config,
    pub config_path: PathBuf,
    pub cwd: &'a Path,
    pub no_system_prompt: bool,
    pub no_tools: bool,
    pub no_subagents: bool,
    pub questionnaire_enabled: bool,
    pub mcp_elicitation: crate::tools::mcp::McpElicitationSupport,
    pub mcp_sampling: McpSamplingSupport,
    pub mcp_attach: super::tools_prompt::McpAttach,
    pub background_subagents: BackgroundSubagents,
    pub diagnostics: &'a RuntimeDiagnostics,
    pub agent: &'a BoundAgent,
    pub max_steps: Option<NonZeroUsize>,
    pub usage_purpose: &'static str,
    pub usage_parent_session_id: Option<rho_sdk::SessionId>,
    pub hook_host_labels: rho_sdk::hooks::HookHostLabels,
    /// Adds caller-owned tools and instructions after shared prompt assembly.
    pub extend_tools_and_prompt: ExtendTools,
    /// Builds the approval wiring for this caller's permission story.
    pub approval: Approval,
    /// Chooses the session options once the provider is known.
    pub session_options: Options,
}

/// Inputs an approval recipe needs; all are produced during assembly.
pub(super) struct ApprovalInputs {
    pub config: Config,
    pub workspace_root: PathBuf,
    pub usage_recording: rho_sdk::ProviderRequestUsageRecording,
    pub session_writes: SessionWriteLog,
}

/// Approval wiring for one session: the handler the runtime enforces and, for
/// hosts that answer prompts themselves, the receiving end of the channel.
pub(super) struct SessionApproval {
    pub session: Option<ApprovalSession>,
    pub receiver: Option<ApprovalRequestReceiver>,
}

/// A live session plus the pieces its owner must shut down.
pub(super) struct BuiltSession {
    pub runtime: rho_sdk::Rho,
    pub session: Session,
    pub provider: Arc<dyn ModelProvider>,
    pub tools: AppToolSet,
    pub hooks: Option<crate::hooks::HookPipeline>,
    pub approval_receiver: Option<ApprovalRequestReceiver>,
}

impl BuiltSession {
    /// Shuts the runtime down first so in-flight work stops before the hook
    /// pipeline drains and the tool hosts close.
    pub(super) async fn teardown(self) {
        let Self {
            runtime,
            session,
            tools,
            hooks,
            ..
        } = self;
        runtime.shutdown();
        drop(session);
        drop(runtime);
        teardown_startup(hooks, tools).await;
    }
}

/// Releases the pieces that exist when startup fails before a session does.
async fn teardown_startup(hooks: Option<crate::hooks::HookPipeline>, tools: AppToolSet) {
    if let Some(hooks) = hooks {
        hooks.shutdown(crate::hooks::DRAIN_GRACE).await;
    }
    tools.shutdown().await;
}

/// An assembled session and the workspace root it resolved to.
pub(super) struct SessionAssembly {
    pub built: BuiltSession,
    pub workspace_root: PathBuf,
}

pub(super) async fn assemble_session<ExtendTools, Approval, Options>(
    options: SessionAssemblyOptions<'_, ExtendTools, Approval, Options>,
) -> anyhow::Result<SessionAssembly>
where
    ExtendTools: FnOnce(AppToolSet, &mut rho_sdk::SystemPrompt) -> AppToolSet,
    Approval: FnOnce(ApprovalInputs) -> anyhow::Result<SessionApproval>,
    Options: FnOnce(Arc<dyn ModelProvider>) -> anyhow::Result<SessionOptions>,
{
    let SessionAssemblyOptions {
        config,
        config_path,
        cwd,
        no_system_prompt,
        no_tools,
        no_subagents,
        questionnaire_enabled,
        mcp_elicitation,
        mcp_sampling,
        mcp_attach,
        background_subagents,
        diagnostics,
        agent,
        max_steps,
        usage_purpose,
        usage_parent_session_id,
        hook_host_labels,
        extend_tools_and_prompt,
        approval,
        session_options,
    } = options;
    // from_config already scopes custom-provider resolution. Do not hold a
    // thread-local overlay across the catalog await: Tokio may resume on
    // another worker and Drop would pop the wrong stack.
    let sdk_options = SdkBootstrapOptions::from_config(config, cwd)?;
    let credentials = ApplicationCredentialSource::new(Arc::new(AppCredentialStore));
    // Automation and ACP normally skip the catalog fetch, but catalog-constructed
    // providers need the hydrate before build; a no-op for the rest.
    sdk_options.provider.ensure_catalog_for_construction().await;
    let provider = build_automation_provider(sdk_options.provider, &credentials)?;
    let live_provider = Arc::clone(&provider);
    let workspace_root = sdk_options.workspace.root.clone();
    let workspace = sdk_options.workspace.build_workspace()?;
    let ToolsAndPrompt {
        tools: tool_set,
        mut system_prompt,
        ..
    } = assemble_tools_and_prompt(ToolsAndPromptOptions {
        config,
        // Subagent and automation assembly can run from a different cwd than
        // bootstrap discovery; let the delegation tools rediscover.
        catalog: None,
        config_path,
        cwd,
        no_system_prompt,
        no_tools,
        no_subagents,
        questionnaire_enabled,
        mcp_elicitation,
        mcp_sampling,
        mcp_attach,
        await_catalog_names: false,
        defer_mcp_connect: false,
        background_subagents,
        diagnostics,
        agent,
    })
    .await?;
    let tool_set = extend_tools_and_prompt(tool_set, &mut system_prompt);

    let context_window = configured_context_window(config);
    let compaction = sdk_options.runtime.compaction.clone();
    diagnostics.update_compaction_config(&compaction);
    let usage_recording = crate::usage::default_recording().await;
    let session_writes = SessionWriteLog::default();
    let SessionApproval {
        session: approval_session,
        receiver: approval_receiver,
    } = approval(ApprovalInputs {
        config: config.clone(),
        workspace_root: workspace_root.clone(),
        usage_recording: usage_recording.clone(),
        session_writes: session_writes.clone(),
    })?;
    let hooks = crate::hooks::start_for_cwd(&workspace_root);
    if let Some(hooks) = hooks.as_ref() {
        diagnostics.attach_hooks(hooks);
    }
    let session_options = match session_options(provider.clone()) {
        Ok(options) => options,
        Err(error) => {
            teardown_startup(hooks, tool_set).await;
            return Err(error);
        }
    };
    let startup_result: anyhow::Result<_> = async {
        let runtime = build_runtime_with_max_steps(
            RuntimeBuildOptions {
                provider: Arc::clone(&provider),
                tools: tool_set.tools(),
                workspace,
                workspace_policy: AppPolicy::for_mode(config.permission_mode, session_writes),
                approval_session,
                system_prompt,
                reasoning: sdk_options.runtime.reasoning,
                service_tier: sdk_options.runtime.service_tier,
                compaction,
                context_window,
                usage_purpose,
                usage_parent_session_id,
                usage_recording,
                hook_host_labels,
                hooks: hooks.as_ref(),
            },
            max_steps,
        )?;
        let session = match runtime.session(session_options).await {
            Ok(session) => session,
            Err(error) => {
                runtime.shutdown();
                return Err(error.into());
            }
        };
        anyhow::Ok((runtime, session))
    }
    .await;
    let (runtime, session) = match startup_result {
        Ok(startup) => startup,
        Err(error) => {
            teardown_startup(hooks, tool_set).await;
            return Err(error);
        }
    };
    if let Some(advisor) = tool_set.advisor() {
        advisor.bind_session(session.clone());
    }
    Ok(SessionAssembly {
        built: BuiltSession {
            runtime,
            session,
            provider: live_provider,
            tools: tool_set,
            hooks,
            approval_receiver,
        },
        workspace_root,
    })
}
