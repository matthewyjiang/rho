use std::{
    io::{self, IsTerminal},
    num::NonZeroUsize,
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use tracing::Instrument;

use {
    crate::cli::{Cli, Command, CredentialStoreCommand, OutputFormat},
    crate::credential_store::AppCredentialStore,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::HerdrReporter,
    crate::session::SessionPersistence,
    crate::tui::SetupEntry,
    crate::update,
    rho_providers::model::ModelError,
};

use super::acp;
use super::{
    agent_binding::{root_host_tools, AgentBinder, AgentInvocation, AgentRole, LaunchTools},
    automation, cli_config,
    config_repository::ConfigRepository,
    daemon_cli, doctor_cli, interactive, login, mcp_cli, plugins_cli,
    sdk_config::SdkBootstrapOptions,
    session_runtime::LeftOut,
    sessions_cli, workflow_cli,
};

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    if !daemon_cli::runs_daemon(cli.command.as_ref()) {
        crate::logging::install_from_env();
    }
    if workflow_cli::planner_worker_requested(&cli) {
        return workflow_cli::run_planner_worker().await;
    }
    let run_output = match &cli.command {
        Some(Command::Run { output, .. }) => Some(*output),
        _ => None,
    };
    let result = run_inner(cli)
        .instrument(tracing::info_span!("startup"))
        .await;
    let Err(error) = result else {
        return Ok(());
    };
    if error.downcast_ref::<automation::AutomationExit>().is_some()
        || error
            .downcast_ref::<automation::AutomationInterrupted>()
            .is_some()
    {
        return Err(error);
    }
    if let Some(output) = run_output {
        let exit = automation::AutomationExit::configuration(error.to_string());
        if output == OutputFormat::Jsonl {
            return Err(automation::emit_startup_failure(exit).into());
        }
        return Err(exit.into());
    }
    Err(error)
}

fn run_inner(cli: Cli) -> impl std::future::Future<Output = anyhow::Result<()>> {
    // Allocate before polling: Box::pin inside `run` alone leaves the large
    // startup construction temporary on its stack throughout the session.
    Box::pin(run_startup(cli))
}

/// Rewrites a `config.toml` rho 2.x wrote to the 3.0 files and says what
/// moved. Runs before anything loads the config, which refuses 2.x keys.
fn upgrade_2x_config(cli: &Cli) -> anyhow::Result<()> {
    let config_path = ConfigRepository::new(cli.config.clone()).configured_path()?;
    let upgraded = crate::config::upgrade_2x(
        &config_path,
        &crate::paths::rho_dir()?,
        crate::credential_store::save_migrated_web_search_keys,
    )
    .with_context(|| format!("could not update 2.x config {}", config_path.display()))?;
    for line in upgraded.iter().flat_map(|done| done.notice(&config_path)) {
        eprintln!("{line}");
    }
    Ok(())
}

async fn run_startup(cli: Cli) -> anyhow::Result<()> {
    cli_config::validate(&cli)?;
    upgrade_2x_config(&cli)?;
    if let EarlyDispatch::Handled(result) = dispatch_early_command(&cli).await? {
        return result;
    }
    if matches!(cli.command, Some(Command::Acp(_))) {
        // `rho acp` is a client of the rho daemon, which runs its sessions.
        return acp::run(cli).await;
    }

    let keyring_notice = crate::credential_store::StartupKeyringNotice::begin(
        /*interactive*/ cli.command.is_none(),
    );
    let PreparedStartup {
        cli,
        catalog,
        config,
        config_repository,
        first_run,
        cwd,
        automation_prompt,
        output_file,
        output,
        max_steps,
        timeout,
        bound_agent,
        bound_reasoning_source,
        provider_refresh,
        added_dirs,
    } = prepare_startup(cli).await?;

    validate_terminal_mode(&cli)?;
    let cli_config::SettledConfig {
        config,
        diagnostics,
    } = cli_config::settle_bound_config(
        config,
        &bound_agent,
        &crate::credential_store::AppCredentialStore,
        &provider_refresh,
        bound_reasoning_source,
    )
    .await?;
    let herdr = HerdrReporter::from_env();
    let headless_herdr = || {
        herdr
            .clone()
            .with_source(crate::herdr::HerdrSource::Headless)
    };
    if let Some(prompt) = automation_prompt {
        return run_automation_startup(AutomationStartup {
            prompt,
            config: &config,
            config_repository: &config_repository,
            cwd,
            cli: &cli,
            bound_agent,
            diagnostics,
            output_file,
            output,
            max_steps,
            timeout,
            herdr: headless_herdr(),
            added_dirs,
        })
        .await;
    }
    run_interactive_startup(InteractiveStartup {
        keyring_notice,
        cli: &cli,
        catalog,
        config,
        config_repository,
        first_run,
        cwd,
        bound_agent,
        bound_reasoning_source,
        diagnostics,
        herdr,
        added_dirs,
    })
    .await
}

enum EarlyDispatch {
    Handled(anyhow::Result<()>),
    Continue,
}

async fn dispatch_early_command(cli: &Cli) -> anyhow::Result<EarlyDispatch> {
    if let Some(Command::ModelPrompt { command }) = &cli.command {
        return Ok(EarlyDispatch::Handled(
            super::model_prompt_cli::run(command, cli).await,
        ));
    }
    if let Some(Command::Uninstall { purge, dry_run }) = &cli.command {
        return Ok(EarlyDispatch::Handled(crate::uninstall::run(
            *purge, *dry_run,
        )));
    }
    if let Some(Command::Workflow { command }) = &cli.command {
        return Ok(EarlyDispatch::Handled(
            workflow_cli::run(command, cli).await,
        ));
    }
    if let Some(Command::CredentialStore { command }) = &cli.command {
        return Ok(EarlyDispatch::Handled(run_credential_store_command(
            command,
            cli.config.clone(),
        )));
    }
    if let Some(Command::Sessions { command }) = &cli.command {
        return Ok(EarlyDispatch::Handled(sessions_cli::run(command)));
    }
    if let Some(Command::Mcp { command }) = &cli.command {
        return Ok(EarlyDispatch::Handled(mcp_cli::run(command, cli).await));
    }
    if let Some(Command::Plugins { command }) = &cli.command {
        return Ok(EarlyDispatch::Handled(plugins_cli::run(command, cli)));
    }
    if let Some(Command::Daemon { command }) = &cli.command {
        return Ok(EarlyDispatch::Handled(
            daemon_cli::run(command.clone()).await,
        ));
    }
    if let Some(Command::Doctor { json }) = &cli.command {
        return Ok(EarlyDispatch::Handled(doctor_cli::run(*json, cli).await));
    }
    #[cfg(debug_assertions)]
    if let Some(Command::AcpFixtureAgent(args)) = &cli.command {
        return Ok(EarlyDispatch::Handled(
            super::acp_fixture_agent::run(args).await,
        ));
    }
    if let Some(Command::Attach { id, output, input }) = &cli.command {
        return Ok(EarlyDispatch::Handled(
            super::attach::run(id, *output, *input).await,
        ));
    }
    if let Some(Command::Run {
        detach: true,
        prompt,
        stdin,
        timeout,
        ..
    }) = &cli.command
    {
        return Ok(EarlyDispatch::Handled(
            super::detach::run(cli, prompt.clone(), *stdin, *timeout).await,
        ));
    }
    if matches!(cli.command, Some(Command::Update)) {
        return Ok(EarlyDispatch::Handled(
            update::run_update(env!("CARGO_PKG_VERSION")).await,
        ));
    }
    if let Some(command) = login::command(&cli.command) {
        // agy owns its sign-in; Rho config and the credential store are not
        // involved.
        let antigravity = |provider: &str| {
            provider.eq_ignore_ascii_case(crate::antigravity_runtime::ANTIGRAVITY_LABEL_NAME)
        };
        match command {
            login::LoginCommand::Cli {
                provider,
                device_auth: true,
            } if antigravity(provider) => anyhow::bail!(
                "antigravity login has no device flow; paste the code agy asks for instead"
            ),
            login::LoginCommand::Cli {
                provider,
                device_auth: false,
            }
            | login::LoginCommand::AcpTerminal { auth: provider }
                if antigravity(provider) =>
            {
                return Ok(EarlyDispatch::Handled(
                    crate::antigravity_runtime::auth::login().await,
                ));
            }
            login::LoginCommand::Cli { .. } | login::LoginCommand::AcpTerminal { .. } => {}
        }
        // Custom providers must be registered for `login` to find them. A
        // daemon login saves under the daemon's config, and asks the daemon
        // which credential store to use (#1662).
        let config_repository = ConfigRepository::new(cli.config.clone());
        let config = config_repository.load()?;
        config.providers.activate()?;
        let signed_in = login::run(command).await;
        return Ok(EarlyDispatch::Handled(signed_in));
    }
    Ok(EarlyDispatch::Continue)
}

struct PreparedStartup {
    cli: Cli,
    catalog: crate::agent::DiscoveredAgentCatalog,
    /// The config to settle (see [`cli_config::settle_bound_config`]): the
    /// bound agent's own when it runs on rho, else the launch config with
    /// CLI overrides.
    config: crate::config::Config,
    config_repository: ConfigRepository,
    first_run: Option<SetupEntry>,
    cwd: std::path::PathBuf,
    automation_prompt: Option<String>,
    output_file: Option<std::path::PathBuf>,
    output: OutputFormat,
    max_steps: Option<NonZeroUsize>,
    timeout: Option<Duration>,
    bound_agent: super::agent_binding::BoundAgent,
    bound_reasoning_source: rho_providers::model::ReasoningRequestSource,
    provider_refresh: cli_config::ProviderRefreshStatus,
    added_dirs: crate::added_dirs::AddedDirs,
}

async fn prepare_startup(cli: Cli) -> anyhow::Result<PreparedStartup> {
    let config_path = cli.config.clone();
    let config_repository = ConfigRepository::new(config_path.clone());
    // Ask before loading; loading writes the default config when none exists.
    let first_run = detect_first_run(&config_repository);
    let mut config = config_repository.load()?;
    // Register every [providers.custom.*] name before refresh, pickers, and /model.
    config.providers.activate()?;
    let absolute_config = absolute_config_path(&config_repository)?;
    crate::credential_store::initialize_from_config(&mut config, &absolute_config)?;
    let cwd = std::env::current_dir()?;
    let added_dirs = crate::added_dirs::launch_added_dirs(&cli.add_dirs, &cwd)?;
    let automation_prompt = automation::prompt_for_command(&cli.command)?;
    let (output_file, output, max_steps, timeout) = match &cli.command {
        Some(Command::Run {
            output_file,
            output,
            max_steps,
            timeout,
            ..
        }) => (output_file.clone(), *output, *max_steps, *timeout),
        _ => (None, OutputFormat::Text, None, None),
    };
    let catalog = Arc::new(crate::agent::AgentCatalog::discover(
        &cwd,
        crate::workspace::ProjectTrusts::of_this_process(),
    )?);
    let selected_agent = cli
        .agent
        .as_deref()
        .unwrap_or(crate::agent::DEFAULT_AGENT_ID);
    let definition = Arc::new(catalog.find(selected_agent)?.definition.clone());
    // The walk is reused for the delegation tool set so startup discovers once.
    let catalog = crate::agent::DiscoveredAgentCatalog::new(cwd.clone(), catalog);

    // Only automation and interactive sessions reach here; every other
    // command dispatches early, and `rho acp` runs in the daemon.
    let role = if automation_prompt.is_some() {
        AgentRole::AutomationRoot
    } else {
        AgentRole::InteractiveRoot
    };

    let store = AppCredentialStore;
    // Interactive sessions refresh custom-provider models after the first frame
    // so a slow host cannot hold up paint. Automation and ACP get one shot to
    // pick a model, so they wait for the session's own host; the rest refresh
    // in the background.
    if matches!(role, AgentRole::AutomationRoot) {
        // The background refresh runs detached.
        let _ =
            cli_config::refresh_custom_provider_models_for_session(&cli, &config, Arc::new(store))
                .await?;
    }
    let provider_refresh = cli_config::refresh_model_cache(&cli, &config, &store).await?;
    let permission_mode_before_override = config.permission_mode;
    let config_changed = cli_config::apply_overrides(&mut config, &cli, &store)?;
    cli_config::prepare_model_metadata(&config, &store, &provider_refresh).await;
    // Full models.dev snapshot fills in the background for subagent and status
    // labels. Interactive sessions stay cache-only on the first frame, then
    // rewrite the startup prompt once if hydrate lands before the first request.
    tokio::spawn(rho_providers::model::models_dev::ensure_models_dev_catalog());
    cli_config::normalize_reasoning(
        &mut config,
        if cli.reasoning.is_some() {
            rho_providers::model::ReasoningRequestSource::Explicit
        } else {
            rho_providers::model::ReasoningRequestSource::PersistedOrDefault
        },
    )?;
    // CLI overrides are session-only unless the user passes --save and an
    // override actually changed the selection. Auto-saving rewrote the whole
    // file and dropped comments. Bare --save, no-op identical overrides, and
    // reasoning auto-normalization alone must not rewrite config.
    if cli.save && config_changed {
        let session_permission_mode = config.permission_mode;
        config.permission_mode = permission_mode_before_override;
        config_repository.save(&config)?;
        config.permission_mode = session_permission_mode;
    }
    let bound_agent = AgentBinder::bind(
        definition,
        AgentInvocation {
            role,
            available_tools: host_capabilities(&cli, &config, role),
        },
        &config,
    )?;
    // An explicit --reasoning stays explicit unless the agent pins its own.
    let bound_reasoning = bound_agent
        .rho_config()
        .map_or(config.reasoning, |bound| bound.reasoning);
    let bound_reasoning_source = if cli.reasoning.is_some() && bound_reasoning == config.reasoning {
        rho_providers::model::ReasoningRequestSource::Explicit
    } else {
        rho_providers::model::ReasoningRequestSource::PersistedOrDefault
    };
    // The bound agent's own config, which may pin a model and reasoning,
    // replaces the launch config; an agent on another runtime keeps it.
    let config = bound_agent.rho_config().cloned().unwrap_or(config);

    Ok(PreparedStartup {
        cli,
        catalog,
        config,
        config_repository,
        first_run,
        cwd,
        automation_prompt,
        output_file,
        output,
        max_steps,
        timeout,
        bound_agent,
        bound_reasoning_source,
        provider_refresh,
        added_dirs,
    })
}

struct AutomationStartup<'a> {
    prompt: String,
    config: &'a crate::config::Config,
    config_repository: &'a ConfigRepository,
    cwd: std::path::PathBuf,
    cli: &'a Cli,
    bound_agent: super::agent_binding::BoundAgent,
    diagnostics: RuntimeDiagnostics,
    output_file: Option<std::path::PathBuf>,
    output: OutputFormat,
    max_steps: Option<NonZeroUsize>,
    timeout: Option<Duration>,
    herdr: HerdrReporter,
    added_dirs: crate::added_dirs::AddedDirs,
}

async fn run_automation_startup(startup: AutomationStartup<'_>) -> anyhow::Result<()> {
    let persistence = match &startup.cli.command {
        Some(Command::Run { no_save: true, .. }) => SessionPersistence::Ephemeral,
        _ => SessionPersistence::Saved,
    };
    automation::run(
        startup.prompt,
        automation::Startup {
            config: startup.config,
            config_path: absolute_config_path(startup.config_repository)?,
            cwd: startup.cwd,
            left_out: launch_left_out(startup.cli),
            agent: startup.bound_agent,
            output_file: startup.output_file,
            output: startup.output,
            max_steps: startup.max_steps,
            timeout: startup.timeout,
            persistence,
            diagnostics: startup.diagnostics,
            herdr: startup.herdr,
            added_dirs: startup.added_dirs,
        },
    )
    .await
}

struct InteractiveStartup<'a> {
    keyring_notice: crate::credential_store::StartupKeyringNotice,
    cli: &'a Cli,
    catalog: crate::agent::DiscoveredAgentCatalog,
    config: crate::config::Config,
    config_repository: ConfigRepository,
    first_run: Option<SetupEntry>,
    cwd: std::path::PathBuf,
    bound_agent: super::agent_binding::BoundAgent,
    bound_reasoning_source: rho_providers::model::ReasoningRequestSource,
    diagnostics: RuntimeDiagnostics,
    herdr: HerdrReporter,
    added_dirs: crate::added_dirs::AddedDirs,
}

async fn run_interactive_startup(startup: InteractiveStartup<'_>) -> anyhow::Result<()> {
    let settings = crate::tui::TuiSettings::load(&crate::paths::rho_dir()?)?;
    let pending_update_notice = settings
        .check_for_updates
        .then(|| tokio::spawn(update::update_notice(env!("CARGO_PKG_VERSION"))));
    let pending_custom_models = cli_config::spawn_custom_provider_models_refresh(
        startup.config.providers.clone(),
        Arc::new(AppCredentialStore),
    );
    let prompt_history_limit = settings.prompt_history_limit;
    let pending_prompt_history = (prompt_history_limit > 0 && !startup.cli.no_save).then(|| {
        tokio::spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    match crate::prompt_history::PromptHistoryStore::at_default_path() {
                        Ok(store) => match store.load_tail(prompt_history_limit) {
                            Ok(tail) => Some((store, tail)),
                            Err(error) => {
                                tracing::warn!(%error, "failed to load prompt history");
                                None
                            }
                        },
                        Err(error) => {
                            tracing::warn!(%error, "failed to open prompt history");
                            None
                        }
                    }
                })
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "prompt history task failed");
                    None
                })
            }
            .instrument(tracing::info_span!("startup.prompt_history")),
        )
    });

    // No thread overlay here: `prepare_startup` already installed these hosts
    // process-wide. An overlay would additionally hide later `/login` additions
    // from every spawned task, since thread-locals do not follow `tokio::spawn`.
    let sdk_options = SdkBootstrapOptions::from_config(&startup.config, &startup.cwd)?;
    let credentials = rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(AppCredentialStore),
    );
    let provider_result = startup
        .config
        .providers
        .require_endpoint_login(&startup.config.provider)
        .and_then(|()| {
            rho_providers::providers::build_sdk_provider_with_source(
                sdk_options.provider,
                &credentials,
            )
        });
    let (missing_auth_error, missing_auth_model_error) = match provider_result {
        Ok(_) => (None, None),
        Err(error) if is_interactive_startup_unavailable_error(&error) => {
            (Some(error.to_string()), Some(error))
        }
        Err(error) => return Err(error.into()),
    };
    interactive::run(interactive::Startup {
        keyring_notice: startup.keyring_notice,
        cli: startup.cli,
        catalog: startup.catalog,
        config: startup.config,
        settings,
        config_path: absolute_config_path(&startup.config_repository)?,
        config_repository: startup.config_repository,
        cwd: startup.cwd,
        first_run: startup.first_run,
        missing_auth_error,
        missing_auth_model_error,
        pending_update_notice,
        pending_custom_models,
        pending_prompt_history,
        diagnostics: startup.diagnostics,
        herdr: startup.herdr,
        agent: startup.bound_agent,
        reasoning_source: startup.bound_reasoning_source,
        added_dirs: startup.added_dirs,
    })
    .await
}

fn run_credential_store_command(
    command: &CredentialStoreCommand,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    match command {
        CredentialStoreCommand::Probe { backend } => {
            let result = crate::credential_store::probe(*backend);
            if result.available {
                println!("available: {}", result.detail);
                Ok(())
            } else {
                anyhow::bail!(result.detail)
            }
        }
        CredentialStoreCommand::Status => {
            // Saved config policy only (ignore RHO_CREDENTIAL_STORE).
            match crate::credential_store::saved_policy_backend(config_path.as_deref())? {
                None => println!("unset"),
                Some(backend) => println!("{}", backend.as_str()),
            }
            Ok(())
        }
        CredentialStoreCommand::Set { backend } => {
            let path = crate::credential_store::set_backend(*backend, config_path)?;
            println!(
                "credential store set to {} in {}",
                backend.as_str(),
                path.display()
            );
            Ok(())
        }
    }
}

/// The parts a `rho run` or `rho acp` launch leaves out of its sessions.
fn launch_left_out(cli: &Cli) -> LeftOut {
    LeftOut {
        system_prompt: cli.no_system_prompt,
        tools: cli.no_tools,
        subagents: cli.no_subagents,
    }
}

pub(super) fn host_capabilities(
    cli: &Cli,
    config: &crate::config::Config,
    role: AgentRole,
) -> crate::agent::AgentCapabilities {
    use crate::agent::ToolCapability;

    let launch = LaunchTools::from_flags(cli.no_tools, cli.no_subagents);
    if launch == LaunchTools::None {
        return crate::agent::AgentCapabilities::default();
    }
    let mut tools = root_host_tools(config, launch);
    if role != AgentRole::InteractiveRoot {
        tools.remove(&ToolCapability::Questionnaire);
    }
    tools
}

pub(super) fn absolute_config_path(
    repository: &ConfigRepository,
) -> anyhow::Result<std::path::PathBuf> {
    let path = repository.configured_path()?;
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Opens the first-run setup screen so the flow can be reviewed without
/// deleting a working config. See [`parse_first_run_override`] for the values.
const FIRST_RUN_OVERRIDE_VAR: &str = "RHO_FIRST_RUN";

/// Which step the override asks for, or `None` when it does not ask at all.
///
/// `signin` and `model` name a step, because a configured machine already has
/// models and would otherwise always land on the model step, leaving the
/// provider menu unreachable. Any other non-empty value that is not `0` means
/// "open setup", letting the step be chosen the way a real first launch does.
fn parse_first_run_override(value: &str) -> Option<SetupEntry> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" | "no" => None,
        "signin" | "sign-in" | "login" => Some(SetupEntry::SignIn),
        "model" | "models" => Some(SetupEntry::ChooseModel),
        _ => Some(SetupEntry::Auto),
    }
}

/// The setup entry for this launch: the override when it is set, otherwise
/// [`SetupEntry::Auto`] when Rho is about to create the config file.
///
/// Call this before loading the config, because loading writes the defaults.
fn detect_first_run(repository: &ConfigRepository) -> Option<SetupEntry> {
    if let Ok(value) = std::env::var(FIRST_RUN_OVERRIDE_VAR) {
        if let Some(entry) = parse_first_run_override(&value) {
            return Some(entry);
        }
    }
    repository
        .configured_path()
        .is_ok_and(|path| !path.exists())
        .then_some(SetupEntry::Auto)
}

fn validate_terminal_mode(cli: &Cli) -> anyhow::Result<()> {
    if cli.command.is_none() && (!crate::stdio::stdin_is_terminal() || !io::stdout().is_terminal())
    {
        anyhow::bail!(
            "rho's default mode is the interactive TUI; use `rho run` for non-interactive automation"
        );
    }
    Ok(())
}

fn is_interactive_startup_unavailable_error(error: &ModelError) -> bool {
    matches!(
        error,
        ModelError::MissingCredentials(_)
            | ModelError::Credentials(_)
            | ModelError::UnsupportedProvider(_)
    )
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
