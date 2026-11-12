use std::path::PathBuf;

use crate::{
    cli::Cli,
    config::Config,
    herdr::HerdrReporter,
    session::{Session, SessionListing, SessionSummary, SessionTarget},
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
    /// The `tui` client settings, theme, and chords.
    pub(super) settings: tui::TuiSettings,
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

// The daemon session an interactive launch follows.
#[path = "interactive_daemon.rs"]
mod daemon;

/// Overrides where the interactive session runs (#1667): `0` keeps it in
/// this process, `1` hosts it in the daemon, which the TUI follows. Read
/// once per launch.
const DAEMON_FLAG: &str = "RHO_TUI_DAEMON";

/// Where an interactive launch's session runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionHost {
    InProcess,
    Daemon,
}

impl SessionHost {
    /// The daemon wherever one runs; Windows has none yet (#1778).
    const PLATFORM_DEFAULT: Self = if cfg!(unix) {
        Self::Daemon
    } else {
        Self::InProcess
    };

    /// The host `flag`, the value of [`DAEMON_FLAG`], asks for: `0` and `1`
    /// choose one, and anything else leaves the platform's default.
    fn from_flag(flag: Option<&std::ffi::OsStr>) -> Self {
        match flag.and_then(std::ffi::OsStr::to_str) {
            Some("0") => Self::InProcess,
            Some("1") => Self::Daemon,
            _ => Self::PLATFORM_DEFAULT,
        }
    }
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
        catalog,
        config,
        settings,
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
    // The one read of the #1667 seam: the session runs in the daemon, which
    // the TUI follows, or in this process. T6 deletes the flag with
    // `InteractiveRuntime` and `tui/prompt_turn.rs`.
    let host = SessionHost::from_flag(std::env::var_os(DAEMON_FLAG).as_deref());
    let daemon = match host {
        SessionHost::InProcess => None,
        SessionHost::Daemon => {
            // Before the daemon builds the session's options from the cache.
            #[cfg(debug_assertions)]
            crate::tui::seed_matrix_model_cache();
            Some(daemon::open(cli, &config, &agent, &startup_session, &cwd, &added_dirs).await?)
        }
    };
    let open_resume_picker = matches!(startup_session, StartupSession::Picker);
    let mut recovered_messages = Vec::new();
    let opened = match startup_session {
        // The daemon holds the session; this process opens none.
        _ if daemon.is_some() => None,
        StartupSession::ById(id) => Some(Session::open_by_id_with_histories(&cwd, id)?),
        StartupSession::Latest(target) => Some(Session::open_target_with_histories(&target)?),
        StartupSession::Picker | StartupSession::Fresh => None,
    };
    if let Some(daemon) = &daemon {
        cwd = daemon.workspace().to_path_buf();
    }
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
        in_process: host == SessionHost::InProcess
            && SessionHost::PLATFORM_DEFAULT == SessionHost::Daemon,
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
        tool_source_attach: match &daemon {
            // The daemon's session connects them.
            Some(_) => super::tools_prompt::ToolSourceAttach::None,
            None => super::tools_prompt::ToolSourceAttach::Connect,
        },
    })
    .await?;
    // Background credential reads must stop writing to stderr before the TUI
    // takes ownership of the terminal.
    drop(keyring_notice);
    let added_dirs = runtime.added_dirs().clone();
    let prompt_cache_ttl = runtime.prompt_cache_ttl();
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
                show_reasoning_output: settings.show_reasoning_output,
                zen_mode: settings.zen_mode,
                advisor_mode: config.advisor_mode,
                cache_miss_notices: settings.cache_miss_notices,
                keep_cache_warm: config.keep_cache_warm,
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
                session_id: match &daemon {
                    Some(daemon) => daemon.saved_id().map(str::to_owned),
                    None => session_id,
                },
                recovered_messages,
                open_resume_picker,
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
