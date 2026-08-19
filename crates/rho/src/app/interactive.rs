use std::path::PathBuf;

use crate::{
    cli::Cli,
    config::Config,
    herdr::HerdrReporter,
    session::Session,
    tui::{self, ApplicationServices, RuntimeModelView, SessionBootstrap, TuiBootstrap},
};

use super::{
    config_repository::ConfigRepository,
    interactive_runtime::{InteractiveRuntime, InteractiveRuntimeOptions},
};

pub(super) struct Startup<'a> {
    pub(super) cli: &'a Cli,
    pub(super) catalog: crate::agent::DiscoveredAgentCatalog,
    pub(super) config: Config,
    pub(super) config_path: PathBuf,
    pub(super) config_repository: ConfigRepository,
    pub(super) cwd: PathBuf,
    pub(super) first_run: Option<crate::tui::SetupEntry>,
    pub(super) missing_auth_error: Option<String>,
    pub(super) missing_auth_model_error: Option<rho_providers::model::ModelError>,
    pub(super) pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pub(super) pending_custom_models: Option<tokio::task::JoinHandle<()>>,
    pub(super) pending_prompt_history: Option<crate::prompt_history::PromptHistoryLoadHandle>,
    pub(super) diagnostics: crate::diagnostics::RuntimeDiagnostics,
    pub(super) herdr: HerdrReporter,
    pub(super) agent: super::agent_binding::BoundAgent,
    pub(super) reasoning_source: rho_providers::model::ReasoningRequestSource,
}

fn validate_resume_agent(
    session: &Session,
    agent: &super::agent_binding::BoundAgent,
) -> anyhow::Result<()> {
    session.validate_agent_definition_identity(agent.definition())
}

pub(super) async fn run(startup: Startup<'_>) -> anyhow::Result<()> {
    let Startup {
        cli,
        catalog,
        config,
        config_path,
        config_repository,
        mut cwd,
        first_run,
        missing_auth_error,
        missing_auth_model_error,
        pending_update_notice,
        pending_custom_models,
        pending_prompt_history,
        diagnostics,
        herdr,
        agent,
        reasoning_source,
    } = startup;
    let mut open_resume_picker = false;
    let mut recovered_messages = Vec::new();
    let (session_id, history, storage) = match &cli.resume {
        Some(Some(id)) => {
            let (session, histories) = Session::open_by_id_with_histories(&cwd, id)?;
            validate_resume_agent(&session, &agent)?;
            cwd = session.cwd().to_path_buf();
            let session_id = Some(session.id().to_string());
            recovered_messages = histories.display;
            (session_id, histories.model, Some(session))
        }
        Some(None) => {
            open_resume_picker = true;
            (None, Vec::new(), None)
        }
        None => (None, Vec::new(), None),
    };
    let pending_syntax_warmup = Some(tui::spawn_syntax_warmup(&recovered_messages));
    let mut prompt_templates = crate::prompt_templates::discover(&cwd);
    crate::prompt_templates::merge(&mut prompt_templates, config.prompt_templates.clone());
    let theme = config.theme.clone();
    let mut runtime = InteractiveRuntime::new(InteractiveRuntimeOptions {
        config: &config,
        catalog: Some(catalog),
        config_path,
        cwd: cwd.clone(),
        no_system_prompt: cli.no_system_prompt,
        no_tools: cli.no_tools,
        no_subagents: cli.no_subagents,
        questionnaire_enabled: !cli.no_tools,
        history,
        session_id: session_id.clone(),
        storage,
        diagnostics: diagnostics.clone(),
        agent,
        unavailable_error: missing_auth_model_error,
    })
    .await?;
    let result = tui::run(
        &mut runtime,
        TuiBootstrap {
            runtime: RuntimeModelView {
                cwd,
                provider: config.provider,
                model: config.model,
                model_aliases: config.model_aliases,
                reasoning: config.reasoning,
                service_tier: config
                    .fast_mode
                    .then_some(rho_sdk::model::ServiceTier::Priority),
                reasoning_source,
                permission_mode: config.permission_mode,
                show_reasoning_output: config.show_reasoning_output,
                zen_mode: config.zen_mode,
                advisor_mode: config.advisor_mode,
                cache_miss_notices: config.cache_miss_notices,
                auth: config.auth,
                internal_agents: config.internal_agents,
                favorite_models: config.favorite_models,
                max_tool_output_lines: config.max_tool_output_lines,
                keybindings: config.keybindings,
                prompt_templates,
            },
            session: SessionBootstrap {
                session_id,
                recovered_messages,
                open_resume_picker,
                startup_prompt: cli.prompt.clone(),
            },
            services: ApplicationServices {
                config_repository,
                theme,
                first_run,
                auth_unavailable: missing_auth_error,
                update_notice: None,
                pending_update_notice,
                pending_custom_models,
                pending_syntax_warmup,
                pending_prompt_history,
                diagnostics,
                herdr,
            },
        },
    )
    .await;
    runtime.shutdown().await;
    let tui_result = result?;
    if let Some(session_id) = tui_result.resume_session_id {
        println!("\nResume this session:\n  rho --resume {session_id}\n");
    }
    Ok(())
}

#[cfg(test)]
#[path = "interactive_tests.rs"]
mod tests;
