//! Resolving what an interactive session starts from.
//!
//! Provider construction, resumed-snapshot selection, prompt cache keys, and the
//! approval channel are all "how do we begin" decisions. Keeping them together
//! leaves the runtime file to the turn loop it actually owns.

use std::{num::NonZeroUsize, path::PathBuf, sync::Arc};

use rho_sdk::{
    model::Message, provider::ModelProvider, ApprovalHandler, ApprovalRequestReceiver, SessionId,
    SessionOptions,
};

use super::{InteractiveRuntime, InteractiveRuntimeOptions};
use crate::{
    app::{
        interactive_run_controller::InteractiveRunController,
        interactive_session_controller::InteractiveSessionController,
        policy::AppPolicy,
        provider_controller::ProviderController,
        runtime_builder::{build_runtime, configured_context_window, RuntimeBuildOptions},
        tools_prompt::{assemble_tools_and_prompt, ToolsAndPrompt, ToolsAndPromptOptions},
    },
    credential_store::AppCredentialStore,
    permission::{remember_allowed_workspace_writes, PermissionMode, SessionWriteLog},
    permission_classifier_handler::ClassifierApprovalHandler,
    session::Session as StoredSession,
    tools::{agent::BackgroundSubagents, sdk_registry::AppToolSet},
};
use rho_providers::providers::{build_sdk_provider_with_source, UnavailableProvider};

pub(super) async fn initialize(
    options: InteractiveRuntimeOptions<'_>,
) -> anyhow::Result<InteractiveRuntime> {
    let InteractiveRuntimeOptions {
        config,
        catalog,
        config_path,
        cwd,
        no_system_prompt,
        no_tools,
        no_subagents,
        questionnaire_enabled,
        history,
        session_id,
        storage,
        diagnostics,
        agent,
        unavailable_error,
    } = options;
    let agent_id = agent.id().to_string();
    let agent_fingerprint = agent.fingerprint().to_string();
    // Warm the usage ledger (sqlite open + migrate) while tools and MCP
    // connect; the await further down then finds the cell already initialized
    // instead of serializing the open behind the rest of startup.
    tokio::spawn(crate::usage::default_recording());
    let sdk_options = crate::app::sdk_config::SdkBootstrapOptions::from_config(config, &cwd)?;
    // Catalog-constructed providers must wait for the models.dev hydrate
    // instead of racing it; a no-op for the rest.
    sdk_options.provider.ensure_catalog_for_construction().await;
    let provider = resolve_provider(unavailable_error, &sdk_options)?;
    let workspace = sdk_options.workspace.build_workspace()?;
    let ToolsAndPrompt {
        tools,
        system_prompt,
        inventory,
        pending_mcp,
        mcp_sampling,
    } = assemble_tools_and_prompt(ToolsAndPromptOptions {
        config,
        catalog: catalog.as_ref(),
        config_path,
        cwd: &cwd,
        no_system_prompt,
        no_tools,
        no_subagents,
        questionnaire_enabled,
        // The interactive host draws questionnaires during a turn, so a server
        // question reaches a person. Without the questionnaire loop it would
        // not, and Rho must not declare a capability it would always decline.
        mcp_elicitation: if questionnaire_enabled {
            crate::tools::mcp::McpElicitationSupport::Available
        } else {
            crate::tools::mcp::McpElicitationSupport::Unavailable
        },
        // Interactive sessions bind a model below, so opted-in servers may ask
        // for completions.
        mcp_sampling: crate::app::tools_prompt::McpSamplingSupport::Available,
        await_catalog_names: false,
        defer_mcp_connect: true,
        background_subagents: BackgroundSubagents::Enabled,
        diagnostics: &diagnostics,
        agent: &agent,
    })
    .await?;
    let mcp_report = inventory.mcp;
    let plugins_report = inventory.plugins;
    let context_window = configured_context_window(config);
    let compaction = sdk_options.runtime.compaction.clone();
    let permission_mode = config.permission_mode;
    diagnostics.update_compaction_config(&compaction);
    let usage_recording = crate::usage::default_recording().await;
    let session_writes = SessionWriteLog::default();
    let approval_channel = approval_channel_for(
        permission_mode,
        ApprovalChannelOptions {
            config: config.clone(),
            workspace_path: workspace.root().to_path_buf(),
            usage_recording: usage_recording.clone(),
            session_writes: session_writes.clone(),
        },
    );
    let hooks = crate::hooks::start_for_cwd(&cwd);
    if let Some(hooks) = hooks.as_ref() {
        diagnostics.attach_hooks(hooks);
    }
    let startup_result: anyhow::Result<_> = async {
        let runtime = build_runtime(RuntimeBuildOptions {
            provider: Arc::clone(&provider),
            tools: tools.tools(),
            workspace: workspace.clone(),
            workspace_policy: AppPolicy::for_mode(permission_mode, session_writes.clone()),
            approval_session: approval_channel
                .handler
                .clone()
                .map(rho_sdk::ApprovalSession::from_shared),
            system_prompt: system_prompt.clone(),
            reasoning: sdk_options.runtime.reasoning,
            service_tier: sdk_options.runtime.service_tier,
            compaction: compaction.clone(),
            context_window,
            usage_purpose: "agent",
            usage_parent_session_id: None,
            usage_recording: usage_recording.clone(),
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
            hooks: hooks.as_ref(),
        })?;
        let session_options = match resolve_session_options(
            &provider,
            history,
            session_id.as_deref(),
            storage.as_ref(),
        ) {
            Ok(options) => options,
            Err(error) => {
                runtime.shutdown();
                return Err(error);
            }
        };
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
            if let Some(hooks) = hooks {
                hooks.shutdown(crate::hooks::DRAIN_GRACE).await;
            }
            tools.shutdown().await;
            return Err(error);
        }
    };
    bind_subagent_parent(&tools, session.id(), storage.as_ref());
    bind_mcp_sampling(&mcp_sampling, &provider, session.id(), &cwd);
    let may_rewrite_startup_prompt =
        storage.is_none() && !matches!(system_prompt, rho_sdk::SystemPrompt::None);
    let pending_catalog_names = may_rewrite_startup_prompt
        .then(|| tokio::spawn(async { rho_providers::model::ensure_model_catalog_names().await }));
    let cached_tool_specs = tools.specs();
    Ok(InteractiveRuntime {
        runtime,
        hooks,
        runs: InteractiveRunController::default(),
        sessions: InteractiveSessionController::new(
            session,
            storage,
            tools.web_access().clone(),
            tools.advisor().cloned(),
        ),
        provider: ProviderController::new(provider, sdk_options.runtime.reasoning),
        tools,
        mcp_sampling,
        mcp_report,
        pending_mcp,
        pending_catalog_names,
        may_rewrite_startup_prompt,
        plugins_report,
        workspace,
        system_prompt,
        compaction,
        pending_compact: None,
        context_window,
        usage_recording,
        config: config.clone(),
        permission_mode,
        experimental_workspace_rewind: config.experimental_workspace_rewind,
        session_writes,
        approval_handler: approval_channel.handler,
        approval_receiver: approval_channel.receiver,
        classifier_approval_handler: approval_channel.classifier,
        agent,
        agent_id,
        agent_fingerprint,
        pending_persistence_error: None,
        pending_persistence_checkpoint: None,
        live_context_warm: false,
        cached_tool_specs,
        tool_list_changed: false,
        completed_runs: 0,
    })
}

/// Hand the live model to MCP sampling.
///
/// Called once the session exists and again whenever the user changes models,
/// because a captured provider would keep spending on the model the user left.
pub(super) fn bind_mcp_sampling(
    bridge: &crate::tools::mcp::McpSamplingBridge,
    provider: &Arc<dyn ModelProvider>,
    session_id: &SessionId,
    workspace_path: &std::path::Path,
) {
    bridge.bind(crate::tools::mcp::McpSamplingModel {
        provider: Arc::clone(provider),
        session_id: session_id.clone(),
        workspace_path: workspace_path.to_path_buf(),
    });
}

pub(super) fn bind_subagent_parent(
    tools: &AppToolSet,
    session_id: &SessionId,
    storage: Option<&StoredSession>,
) {
    if let Some(manager) = tools.subagents() {
        manager.bind_parent_session(crate::subagent::RunPlacement::for_parent_session(
            session_id.to_string(),
            storage.and_then(StoredSession::subagents_dir),
        ));
    }
    tools
        .workflow_tracker()
        .bind_parent_session(session_id.to_string());
}

pub(super) fn resolve_provider(
    unavailable_error: Option<rho_providers::model::ModelError>,
    sdk_options: &crate::app::sdk_config::SdkBootstrapOptions,
) -> anyhow::Result<Arc<dyn ModelProvider>> {
    match unavailable_error {
        Some(error) => Ok(Arc::new(UnavailableProvider::new(error))),
        None => {
            let credentials =
                rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
                    Arc::new(AppCredentialStore),
                );
            Ok(build_sdk_provider_with_source(
                sdk_options.provider.clone(),
                &credentials,
            )?)
        }
    }
}

pub(super) fn resolve_session_options(
    provider: &Arc<dyn ModelProvider>,
    history: Vec<Message>,
    session_id: Option<&str>,
    storage: Option<&StoredSession>,
) -> anyhow::Result<SessionOptions> {
    let cache_key = session_id.map(prompt_cache_key);
    let resumed_snapshot = storage
        .map(|storage| {
            storage.snapshot_for_resume(
                provider.identity(),
                cache_key
                    .clone()
                    .unwrap_or_else(|| prompt_cache_key(storage.id())),
            )
        })
        .transpose()?;
    if let Some(snapshot) = resumed_snapshot {
        // The TUI has not started yet, so stderr is still safe here.
        if let Some(notice) = resume_omissions_notice(&snapshot, &provider.identity()) {
            eprintln!("warning: {notice}");
        }
        return Ok(SessionOptions::from_snapshot(snapshot));
    }
    // Always seed a prompt-cache key, including brand-new sessions that
    // do not yet have durable storage. ensure_session later reuses this
    // session id when creating the on-disk transcript.
    let id = match session_id {
        Some(id) => SessionId::from_string(id)?,
        None => SessionId::new(),
    };
    Ok(session_options_for_id(id).history(history))
}

pub(in crate::app) struct ApprovalChannelOptions {
    pub(in crate::app) config: crate::config::Config,
    pub(in crate::app) workspace_path: PathBuf,
    pub(in crate::app) usage_recording: rho_sdk::ProviderRequestUsageRecording,
    pub(in crate::app) session_writes: SessionWriteLog,
}

pub(in crate::app) struct ApprovalChannel {
    pub(in crate::app) handler: Option<Arc<dyn ApprovalHandler>>,
    pub(in crate::app) receiver: Option<ApprovalRequestReceiver>,
    pub(in crate::app) classifier: Option<Arc<ClassifierApprovalHandler>>,
}

pub(in crate::app) fn approval_channel_for(
    mode: PermissionMode,
    options: ApprovalChannelOptions,
) -> ApprovalChannel {
    let ApprovalChannel {
        handler,
        receiver,
        classifier,
    } = match mode {
        PermissionMode::Auto => {
            let capacity = NonZeroUsize::new(16).expect("approval channel capacity is non-zero");
            let (human_handler, receiver) = rho_sdk::approval_channel(capacity);
            // Pass the raw human channel. The classifier records both its own
            // allows and human escalations into `session_writes`, so a later
            // isolate_for_run cannot leak grants into the template's log.
            let classifier = ClassifierApprovalHandler::shared(
                options.config,
                options.workspace_path,
                options.usage_recording,
                Some(Arc::new(human_handler)),
                Some(options.session_writes.clone()),
            );
            ApprovalChannel {
                handler: Some(classifier.clone()),
                receiver: Some(receiver),
                classifier: Some(classifier),
            }
        }
        PermissionMode::AllowEdits | PermissionMode::Supervised => {
            let capacity = NonZeroUsize::new(16).expect("approval channel capacity is non-zero");
            let (handler, receiver) = rho_sdk::approval_channel(capacity);
            ApprovalChannel {
                handler: Some(Arc::new(handler)),
                receiver: Some(receiver),
                classifier: None,
            }
        }
        PermissionMode::Bypass | PermissionMode::Plan => ApprovalChannel {
            handler: None,
            receiver: None,
            classifier: None,
        },
    };
    // Allow edits records human approvals. Auto records classifier allows and
    // human escalations on the classifier handler itself. Supervised keeps
    // every write behind the gate, so its handler stays unwrapped.
    let handler = match (handler, mode) {
        (Some(handler), PermissionMode::AllowEdits) => Some(remember_allowed_workspace_writes(
            handler,
            options.session_writes,
            crate::permission::WriteAuthority::Human,
        )),
        (other, _) => other,
    };
    ApprovalChannel {
        handler,
        receiver,
        classifier,
    }
}

pub(in crate::app) fn prompt_cache_key(id: &str) -> String {
    rho_providers::providers::openai::prompt_cache_key_from_session_id(id)
        .unwrap_or_else(|| format!("rho:{id}"))
}

/// New session options with a stable prompt-cache key derived from the id.
pub(in crate::app) fn session_options_for_id(id: SessionId) -> SessionOptions {
    let key = prompt_cache_key(id.as_str());
    SessionOptions::new().id(id).prompt_cache_key(key)
}

pub(in crate::app) fn fresh_session_options() -> SessionOptions {
    session_options_for_id(SessionId::new())
}

pub(super) fn resume_omissions_report(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) -> Option<rho_sdk::model::handoff::HandoffReport> {
    let report = snapshot.provider_context_omissions(target);
    report.has_omissions().then_some(report)
}

fn resume_omissions_notice(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) -> Option<String> {
    resume_omissions_report(snapshot, target).map(|report| {
        format!(
            "omitted {} incompatible provider-native context block(s) while resuming session (kinds: {})",
            report.omitted_provider_context,
            report.omitted_kinds.join(", ")
        )
    })
}
