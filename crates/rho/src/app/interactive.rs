use std::path::PathBuf;

use crate::{
    cli::Cli,
    config::Config,
    herdr::HerdrReporter,
    session::{Session, SessionListing, SessionSummary, SessionTarget},
    tui::{self, ApplicationServices, RuntimeModelView, SessionBootstrap, TuiBootstrap},
};

use super::config_repository::ConfigRepository;

pub(super) struct Startup<'a> {
    pub(super) keyring_notice: crate::credential_store::StartupKeyringNotice,
    pub(super) cli: &'a Cli,
    pub(super) config: Config,
    /// The `tui` client settings, theme, and chords.
    pub(super) settings: tui::TuiSettings,
    pub(super) config_repository: ConfigRepository,
    pub(super) cwd: PathBuf,
    pub(super) first_run: Option<crate::tui::SetupEntry>,
    pub(super) missing_auth_error: Option<String>,
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

// The daemon session an interactive launch follows.
#[path = "interactive_daemon.rs"]
mod daemon;

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
            let latest = Session::list(cwd, SessionListing::PersonFacing)?
                .into_iter()
                .find(SessionSummary::has_messages);
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
        config,
        settings,
        config_repository,
        cwd,
        first_run,
        missing_auth_error,
        pending_update_notice,
        pending_custom_models,
        pending_prompt_history,
        diagnostics,
        herdr,
        agent,
        reasoning_source,
        added_dirs,
    } = startup;
    // Name the workspace as the daemon does, canonicalized, so `--resume`
    // and `--continue` look in the directory the daemon saves its sessions
    // in. A short (8.3) Windows path otherwise names another workspace, and
    // `--resume` falls back to the all-workspace index, which can miss a
    // session whose turn the daemon is finishing (#1915).
    let cwd = crate::paths::canonicalize(&cwd).unwrap_or(cwd);
    let startup_session = StartupSession::from_cli(cli, &cwd)?;
    // Before the daemon builds the session's options from the cache.
    #[cfg(debug_assertions)]
    crate::tui::seed_matrix_model_cache();
    let daemon = daemon::open(cli, &config, &agent, &startup_session, &cwd, &added_dirs).await?;
    let cwd = daemon.workspace().to_path_buf();
    let resume_launch = tui::ResumeLaunchOptions {
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
    // Background credential reads must stop writing to stderr before the TUI
    // takes ownership of the terminal.
    drop(keyring_notice);
    // Taken after this process's own upgrade and `daemon::open`, so an
    // upgrade this or any earlier rho process did (a daemon, `rho acp`)
    // shows here. An unknown home only means no note.
    let config_upgrade_note = crate::paths::rho_dir()
        .ok()
        .and_then(|rho_dir| crate::config::take_upgrade_note(&rho_dir));
    let prompt_cache_ttl = config.providers.resolved_prompt_cache_ttl(&config.provider);
    let result = tui::run(TuiBootstrap {
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
            show_reasoning_output: settings.show_reasoning_output,
            zen_mode: settings.zen_mode,
            advisor_mode: config.advisor_mode,
            cache_miss_notices: settings.cache_miss_notices,
            prompt_cache_ttl,
            show_header_hints: settings.show_header_hints,
            notifications: settings.notifications,
            auth: config.auth,
            internal_agents: config.internal_agents,
            favorite_models: config.favorite_models,
            max_tool_output_lines: settings.max_tool_output_lines,
            keybindings: settings.keybindings.clone(),
            config_prompt_templates: config.prompt_templates,
            prompt_template_home: crate::paths::home_dir(),
        },
        session: SessionBootstrap {
            no_save: cli.no_save,
            session_id: daemon.saved_id().map(str::to_owned),
            open_resume_picker: matches!(startup_session, StartupSession::Picker),
            resume_launch,
            startup_prompt: cli.prompt.clone(),
            daemon,
        },
        services: ApplicationServices {
            config_repository,
            settings,
            first_run,
            auth_unavailable: missing_auth_error,
            update_notice: None,
            config_upgrade_note,
            pending_update_notice,
            pending_custom_models,
            pending_syntax_warmup: Some(tui::spawn_syntax_warmup(&[])),
            pending_prompt_history,
            diagnostics,
            herdr,
        },
    })
    .await;
    tui::print_exit_receipt(result?.as_ref())?;
    Ok(())
}

#[cfg(test)]
#[path = "interactive_tests.rs"]
mod tests;
