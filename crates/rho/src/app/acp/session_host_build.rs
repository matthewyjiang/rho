use std::{num::NonZeroUsize, path::Path, sync::Arc};

use agent_client_protocol::schema::v1::SessionModeState;
use rho_providers::{
    auth::provider_credentials::ApplicationCredentialSource, providers::build_automation_provider,
};
use rho_sdk::{
    provider::ModelProvider, ApprovalHandler, ApprovalRequestReceiver, ApprovalSession, Session,
    SessionOptions,
};

use super::super::permission;
use super::SessionBuildContext;
use crate::{
    app::{
        automation::ensure_headless_auto_classifier_model,
        policy::AppPolicy,
        runtime_builder::{
            build_runtime_with_max_steps, configured_context_window, RuntimeBuildOptions,
        },
        sdk_config::SdkBootstrapOptions,
        tools_prompt::{
            assemble_tools_and_prompt, McpSamplingSupport, ToolsAndPrompt, ToolsAndPromptOptions,
        },
    },
    credential_store::AppCredentialStore,
    permission::{remember_allowed_workspace_writes, PermissionMode, SessionWriteLog},
    permission_classifier_handler::ClassifierApprovalHandler,
    tools::{agent::BackgroundSubagents, sdk_registry::AppToolSet},
};

pub(super) struct BuiltSession {
    pub runtime: rho_sdk::Rho,
    pub session: Session,
    pub tools: AppToolSet,
    pub hooks: Option<crate::hooks::HookPipeline>,
    pub approval_receiver: Option<ApprovalRequestReceiver>,
}

pub(super) async fn build_session(
    ctx: &SessionBuildContext<'_>,
    workspace: &Path,
    session_options: impl FnOnce(&dyn ModelProvider) -> anyhow::Result<SessionOptions>,
) -> anyhow::Result<BuiltSession> {
    ensure_headless_auto_classifier_model(ctx.config)?;
    let _scope = ctx.config.providers.thread_scope()?;
    let sdk_options = SdkBootstrapOptions::from_config(ctx.config, workspace)?;
    let credentials = ApplicationCredentialSource::new(Arc::new(AppCredentialStore));
    let provider = build_automation_provider(sdk_options.provider.clone(), &credentials)?;
    let workspace_root = sdk_options.workspace.root.clone();
    let built_workspace = sdk_options.workspace.build_workspace()?;
    let ToolsAndPrompt {
        tools: tool_set,
        system_prompt,
        ..
    } = assemble_tools_and_prompt(ToolsAndPromptOptions {
        config: ctx.config,
        config_path: ctx.config_path.to_path_buf(),
        cwd: workspace,
        no_system_prompt: ctx.no_system_prompt,
        no_tools: ctx.no_tools,
        no_subagents: ctx.no_subagents,
        questionnaire_enabled: false,
        mcp_elicitation: crate::tools::mcp::McpElicitationSupport::Unavailable,
        mcp_sampling: McpSamplingSupport::Unavailable,
        await_catalog_names: false,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: ctx.diagnostics,
        agent: ctx.agent,
    })
    .await?;

    let context_window = configured_context_window(ctx.config);
    let compaction = sdk_options.runtime.compaction.clone();
    ctx.diagnostics.update_compaction_config(&compaction);
    let usage_recording = crate::usage::default_recording().await;
    let session_writes = SessionWriteLog::default();
    let approval = approval_channel_for(
        ctx.config.permission_mode,
        ctx.config.clone(),
        workspace_root.clone(),
        usage_recording.clone(),
        session_writes.clone(),
    );
    let hooks = crate::hooks::start_for_cwd(&workspace_root);
    if let Some(hooks) = hooks.as_ref() {
        ctx.diagnostics.attach_hooks(hooks);
    }
    let session_options = match session_options(provider.as_ref()) {
        Ok(options) => options,
        Err(error) => {
            teardown_startup(hooks, tool_set).await;
            return Err(error);
        }
    };
    let startup_result: anyhow::Result<_> = async {
        let runtime = build_runtime_with_max_steps(
            RuntimeBuildOptions {
                provider,
                tools: tool_set.tools(),
                workspace: built_workspace,
                workspace_policy: AppPolicy::for_mode(ctx.config.permission_mode, session_writes),
                approval_session: approval.handler.clone().map(ApprovalSession::from_shared),
                system_prompt,
                reasoning: sdk_options.runtime.reasoning,
                service_tier: sdk_options.runtime.service_tier,
                compaction,
                context_window,
                usage_purpose: "agent",
                usage_parent_session_id: None,
                usage_recording,
                hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
                hooks: hooks.as_ref(),
            },
            None,
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
    Ok(BuiltSession {
        runtime,
        session,
        tools: tool_set,
        hooks,
        approval_receiver: approval.receiver,
    })
}

pub(super) fn prompt_cache_key(id: &str) -> String {
    rho_providers::providers::openai::prompt_cache_key_from_session_id(id)
        .unwrap_or_else(|| format!("rho:{id}"))
}

pub(super) fn mode_state(mode: PermissionMode) -> SessionModeState {
    SessionModeState::new(permission::mode_id(mode), permission::mode_list())
}

pub(super) async fn teardown_session(
    runtime: rho_sdk::Rho,
    session: Session,
    hooks: Option<crate::hooks::HookPipeline>,
    tools: AppToolSet,
) {
    runtime.shutdown();
    drop(session);
    drop(runtime);
    if let Some(hooks) = hooks {
        hooks.shutdown(crate::hooks::DRAIN_GRACE).await;
    }
    tools.shutdown().await;
}

async fn teardown_startup(hooks: Option<crate::hooks::HookPipeline>, tools: AppToolSet) {
    if let Some(hooks) = hooks {
        hooks.shutdown(crate::hooks::DRAIN_GRACE).await;
    }
    tools.shutdown().await;
}

struct ApprovalChannel {
    handler: Option<Arc<dyn ApprovalHandler>>,
    receiver: Option<ApprovalRequestReceiver>,
}

/// Same approval recipe as interactive startup: Supervised/AllowEdits get a
/// raw channel, Auto wraps the human channel in the classifier, Bypass/Plan
/// have none.
fn approval_channel_for(
    mode: PermissionMode,
    config: crate::config::Config,
    workspace_path: std::path::PathBuf,
    usage_recording: rho_sdk::ProviderRequestUsageRecording,
    session_writes: SessionWriteLog,
) -> ApprovalChannel {
    let (handler, receiver) = match mode {
        PermissionMode::Auto => {
            let capacity = NonZeroUsize::new(16).expect("approval channel capacity is non-zero");
            let (human_handler, receiver) = rho_sdk::approval_channel(capacity);
            let classifier = ClassifierApprovalHandler::shared(
                config,
                workspace_path,
                usage_recording,
                Some(Arc::new(human_handler)),
                Some(session_writes.clone()),
            );
            (Some(classifier as Arc<dyn ApprovalHandler>), Some(receiver))
        }
        PermissionMode::AllowEdits | PermissionMode::Supervised => {
            let capacity = NonZeroUsize::new(16).expect("approval channel capacity is non-zero");
            let (handler, receiver) = rho_sdk::approval_channel(capacity);
            (
                Some(Arc::new(handler) as Arc<dyn ApprovalHandler>),
                Some(receiver),
            )
        }
        PermissionMode::Bypass | PermissionMode::Plan => (None, None),
    };
    let handler = match (handler, mode) {
        (Some(handler), PermissionMode::AllowEdits) => Some(remember_allowed_workspace_writes(
            handler,
            session_writes,
            crate::permission::WriteAuthority::Human,
        )),
        (other, _) => other,
    };
    ApprovalChannel { handler, receiver }
}
