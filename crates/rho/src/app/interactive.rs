use std::path::PathBuf;

use crate::{
    cli::Cli,
    config::Config,
    herdr::HerdrReporter,
    session::{Session, SessionTarget},
    tui::{self, ApplicationServices, RuntimeModelView, SessionBootstrap, TuiBootstrap},
};

use super::{
    config_repository::ConfigRepository,
    interactive_runtime::{InteractiveRuntime, InteractiveRuntimeOptions},
};

pub(super) struct Startup<'a> {
    pub(super) keyring_notice: crate::credential_store::StartupKeyringNotice,
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
    /// `--add-dir` directories, granted to every session this process opens.
    pub(super) added_dirs: crate::added_dirs::AddedDirs,
}

fn validate_resume_agent(
    session: &Session,
    agent: &super::agent_binding::BoundAgent,
) -> anyhow::Result<()> {
    session.validate_agent_definition_identity(agent.definition())
}

/// Session an interactive launch starts in, chosen from `--resume` / `--continue`.
enum StartupSession<'a> {
    Fresh,
    Picker,
    /// `--resume <ID>`: local-then-global id prefix resolution.
    ById(&'a str),
    /// `--continue`: the newest session in the current workspace.
    Latest(SessionTarget),
}

impl<'a> StartupSession<'a> {
    fn from_cli(cli: &'a Cli, cwd: &std::path::Path) -> anyhow::Result<Self> {
        if cli.continue_latest {
            // An empty workspace starts fresh so `rho -c` is always safe to run.
            let latest = Session::list(cwd)?.into_iter().next();
            return Ok(latest.map_or(Self::Fresh, |summary| Self::Latest(summary.target())));
        }
        Ok(match &cli.resume {
            Some(Some(id)) => Self::ById(id),
            Some(None) => Self::Picker,
            None => Self::Fresh,
        })
    }
}

pub(super) fn run(
    startup: Startup<'_>,
) -> impl std::future::Future<Output = anyhow::Result<()>> + '_ {
    // Construct and box outside the caller's poll frame. Boxing inside an async
    // caller still reserves its large debug move temporary while polling input.
    Box::pin(run_inner(startup))
}

async fn run_inner(startup: Startup<'_>) -> anyhow::Result<()> {
    let Startup {
        keyring_notice,
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
        added_dirs,
    } = startup;
    let startup_session = StartupSession::from_cli(cli, &cwd)?;
    let open_resume_picker = matches!(startup_session, StartupSession::Picker);
    let mut recovered_messages = Vec::new();
    let opened = match startup_session {
        StartupSession::ById(id) => Some(Session::open_by_id_with_histories(&cwd, id)?),
        StartupSession::Latest(target) => Some(Session::open_target_with_histories(&target)?),
        StartupSession::Picker | StartupSession::Fresh => None,
    };
    let (session_id, history, storage) = match opened {
        Some((session, histories)) => {
            validate_resume_agent(&session, &agent)?;
            cwd = session.cwd().to_path_buf();
            let session_id = Some(session.id().to_string());
            recovered_messages = histories.display;
            (session_id, histories.model, Some(session))
        }
        None => (None, Vec::new(), None),
    };
    let pending_syntax_warmup = Some(tui::spawn_syntax_warmup(&recovered_messages));
    let theme = config.theme.clone();
    let resume_launch = tui::ResumeLaunchOptions {
        config: cli.config.is_some().then(|| config_path.clone()),
        agent: cli.agent.clone(),
        // Only `--agent` is replayed, so only its binding is part of the baseline.
        definition: cli
            .agent
            .is_some()
            .then(|| std::sync::Arc::new(agent.definition().clone())),
        no_system_prompt: cli.no_system_prompt,
        no_tools: cli.no_tools,
        no_subagents: cli.no_subagents,
    };
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
        launch_added_dirs: added_dirs,
    })
    .await?;
    // Background credential reads must stop writing to stderr before the TUI
    // takes ownership of the terminal.
    drop(keyring_notice);
    let added_dirs = runtime.added_dirs().clone();
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
                added_dirs,
                show_reasoning_output: config.show_reasoning_output,
                zen_mode: config.zen_mode,
                advisor_mode: config.advisor_mode,
                cache_miss_notices: config.cache_miss_notices,
                show_header_hints: config.show_header_hints,
                notifications: config.notifications,
                auth: config.auth,
                internal_agents: config.internal_agents,
                favorite_models: config.favorite_models,
                max_tool_output_lines: config.max_tool_output_lines,
                keybindings: config.keybindings,
                config_prompt_templates: config.prompt_templates,
                prompt_template_home: crate::paths::home_dir(),
            },
            session: SessionBootstrap {
                no_save: cli.no_save,
                session_id,
                recovered_messages,
                open_resume_picker,
                resume_launch,
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
    tui::print_exit_receipt(result?.as_ref())?;
    Ok(())
}

#[cfg(test)]
#[path = "interactive_tests.rs"]
mod tests;
