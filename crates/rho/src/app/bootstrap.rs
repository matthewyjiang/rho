use std::{
    io::{self, IsTerminal},
    num::NonZeroUsize,
    sync::Arc,
    time::Duration,
};

use {
    crate::cli::{Cli, Command, CredentialStoreCommand, OutputFormat},
    crate::credential_store::AppCredentialStore,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::HerdrReporter,
    crate::tui::SetupEntry,
    crate::update,
    rho_providers::model::ModelError,
};

use super::{
    acp,
    agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    automation, automation_protocol, cli_config,
    config_repository::ConfigRepository,
    interactive, login, mcp_cli, plugins_cli,
    sdk_config::SdkBootstrapOptions,
    sessions_cli, workflow_cli,
};

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    if workflow_cli::planner_worker_requested(&cli) {
        return workflow_cli::run_planner_worker().await;
    }
    let run_output = match &cli.command {
        Some(Command::Run { output, .. }) => Some(*output),
        _ => None,
    };
    let result = run_inner(cli).await;
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
    if run_output == Some(OutputFormat::Jsonl) {
        let message = error.to_string();
        automation::emit_startup_failure(message.clone())?;
        return Err(automation::AutomationExit::new(
            2,
            automation_protocol::TerminalReason::ConfigurationError,
            message,
        )
        .into());
    }
    if run_output.is_some() {
        return Err(automation::AutomationExit::new(
            2,
            automation_protocol::TerminalReason::ConfigurationError,
            error.to_string(),
        )
        .into());
    }
    Err(error)
}

async fn run_inner(cli: Cli) -> anyhow::Result<()> {
    cli_config::validate(&cli)?;
    if let EarlyDispatch::Handled(result) = dispatch_early_command(&cli).await? {
        return result;
    }

    let PreparedStartup {
        cli,
        catalog,
        mut config,
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
        store,
    } = prepare_startup(cli).await?;

    validate_terminal_mode(&cli)?;
    cli_config::prepare_model_metadata(&config, &store, &provider_refresh).await;
    cli_config::normalize_reasoning_for_cli(&mut config, bound_reasoning_source)?;
    let herdr = HerdrReporter::from_env();
    if let Some(prompt) = automation_prompt {
        return run_automation_startup(AutomationStartup {
            prompt,
            config: &config,
            config_repository: &config_repository,
            cwd,
            cli: &cli,
            bound_agent,
            output_file,
            output,
            max_steps,
            timeout,
            herdr,
        })
        .await;
    }
    if matches!(cli.command, Some(Command::Acp)) {
        return run_acp_startup(AcpCommandStartup {
            config,
            config_repository,
            cwd,
            cli,
            bound_agent,
            herdr,
        })
        .await;
    }
    run_interactive_startup(InteractiveStartup {
        cli: &cli,
        catalog,
        config,
        config_repository,
        first_run,
        cwd,
        bound_agent,
        bound_reasoning_source,
        herdr,
    })
    .await
}

enum EarlyDispatch {
    Handled(anyhow::Result<()>),
    Continue,
}

async fn dispatch_early_command(cli: &Cli) -> anyhow::Result<EarlyDispatch> {
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
    if let Some(Command::Attach { id }) = &cli.command {
        // Attach is early-dispatched, so load display settings here the way the
        // interactive TUI gets them through RuntimeModelView. Propagate load
        // failures instead of failing open to full reasoning / work chrome.
        let display = crate::tui::AttachmentDisplaySettings::from_config(
            &ConfigRepository::new(cli.config.clone()).load()?,
        );
        return Ok(EarlyDispatch::Handled(
            crate::tui::run_attachment(id.as_deref(), display, HerdrReporter::from_env()).await,
        ));
    }
    if matches!(cli.command, Some(Command::Update)) {
        return Ok(EarlyDispatch::Handled(
            update::run_update(env!("CARGO_PKG_VERSION")).await,
        ));
    }
    if let Some(Command::Login {
        provider,
        device_auth,
    }) = &cli.command
    {
        let config_repository = ConfigRepository::new(cli.config.clone());
        let mut config = config_repository.load()?;
        let config_path = absolute_config_path(&config_repository)?;
        ensure_cli_credential_store_choice(&mut config, Some(config_path.clone()))?;
        crate::credential_store::initialize_from_config(&mut config, &config_path)?;
        return Ok(EarlyDispatch::Handled(
            login::run(provider, *device_auth).await,
        ));
    }
    Ok(EarlyDispatch::Continue)
}

struct PreparedStartup {
    cli: Cli,
    catalog: crate::agent::DiscoveredAgentCatalog,
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
    store: AppCredentialStore,
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
    let catalog = Arc::new(crate::agent::AgentCatalog::discover(&cwd)?);
    let selected_agent = cli.agent.as_deref().unwrap_or("default");
    let definition = Arc::new(catalog.find(selected_agent)?.definition.clone());
    // The walk is reused for the delegation tool set so startup discovers once.
    let catalog = crate::agent::DiscoveredAgentCatalog::new(cwd.clone(), catalog);

    let store = AppCredentialStore;
    cli_config::refresh_custom_provider_models(&config, &store).await;
    let provider_refresh = cli_config::refresh_model_cache(&cli, &config, &store).await?;
    let permission_mode_before_override = config.permission_mode;
    let config_changed = cli_config::apply_overrides(&mut config, &cli)?;
    cli_config::prepare_model_metadata(&config, &store, &provider_refresh).await;
    // Full models.dev snapshot fills in the background for subagent and status
    // labels. The interactive system prompt awaits the same hydrate before it
    // prints permanent model lines; see `await_catalog_names` on tools assembly.
    tokio::spawn(rho_providers::model::models_dev::ensure_models_dev_catalog());
    cli_config::normalize_reasoning_for_cli(
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
    let reasoning_before_binding = config.reasoning;
    let role = if automation_prompt.is_some() || matches!(cli.command, Some(Command::Acp)) {
        AgentRole::AutomationRoot
    } else {
        AgentRole::InteractiveRoot
    };
    let bound_agent = AgentBinder::bind(
        definition,
        AgentInvocation {
            role,
            available_tools: host_capabilities(&cli, &config, role),
        },
        &config,
    )?;
    config = bound_agent.rho_config().cloned().unwrap_or(config);
    let bound_reasoning_source =
        if cli.reasoning.is_some() && config.reasoning == reasoning_before_binding {
            rho_providers::model::ReasoningRequestSource::Explicit
        } else {
            rho_providers::model::ReasoningRequestSource::PersistedOrDefault
        };

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
        store,
    })
}

struct AutomationStartup<'a> {
    prompt: String,
    config: &'a crate::config::Config,
    config_repository: &'a ConfigRepository,
    cwd: std::path::PathBuf,
    cli: &'a Cli,
    bound_agent: super::agent_binding::BoundAgent,
    output_file: Option<std::path::PathBuf>,
    output: OutputFormat,
    max_steps: Option<NonZeroUsize>,
    timeout: Option<Duration>,
    herdr: HerdrReporter,
}

async fn run_automation_startup(startup: AutomationStartup<'_>) -> anyhow::Result<()> {
    let diagnostics = bind_agent_diagnostics(startup.config, &startup.bound_agent);
    automation::run(
        startup.prompt,
        automation::Startup {
            config: startup.config,
            config_path: absolute_config_path(startup.config_repository)?,
            cwd: startup.cwd,
            no_system_prompt: startup.cli.no_system_prompt,
            no_tools: startup.cli.no_tools,
            no_subagents: startup.cli.no_subagents,
            usage_purpose: "agent",
            parent_session_id: None,
            agent: startup.bound_agent,
            output_file: startup.output_file,
            output: startup.output,
            max_steps: startup.max_steps,
            timeout: startup.timeout,
            diagnostics,
            herdr: startup.herdr,
            host_input: None,
            notice_poster: None,
            steering_slot: None,
            approval_session: None,
            approval_classifier: None,
            hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        },
    )
    .await
}

struct AcpCommandStartup {
    config: crate::config::Config,
    config_repository: ConfigRepository,
    cwd: std::path::PathBuf,
    cli: Cli,
    bound_agent: super::agent_binding::BoundAgent,
    herdr: HerdrReporter,
}

async fn run_acp_startup(startup: AcpCommandStartup) -> anyhow::Result<()> {
    let diagnostics = bind_agent_diagnostics(&startup.config, &startup.bound_agent);
    acp::run(acp::AcpStartup {
        config: startup.config,
        config_path: absolute_config_path(&startup.config_repository)?,
        cwd: startup.cwd,
        no_system_prompt: startup.cli.no_system_prompt,
        no_tools: startup.cli.no_tools,
        no_subagents: startup.cli.no_subagents,
        agent: startup.bound_agent,
        diagnostics,
        herdr: startup.herdr,
    })
    .await
}

struct InteractiveStartup<'a> {
    cli: &'a Cli,
    catalog: crate::agent::DiscoveredAgentCatalog,
    config: crate::config::Config,
    config_repository: ConfigRepository,
    first_run: Option<SetupEntry>,
    cwd: std::path::PathBuf,
    bound_agent: super::agent_binding::BoundAgent,
    bound_reasoning_source: rho_providers::model::ReasoningRequestSource,
    herdr: HerdrReporter,
}

async fn run_interactive_startup(startup: InteractiveStartup<'_>) -> anyhow::Result<()> {
    let diagnostics = bind_agent_diagnostics(&startup.config, &startup.bound_agent);

    let pending_update_notice = startup
        .config
        .check_for_updates
        .then(|| tokio::spawn(update::update_notice(env!("CARGO_PKG_VERSION"))));

    let _scope = startup.config.providers.thread_scope()?;
    let sdk_options = SdkBootstrapOptions::from_config(&startup.config, &startup.cwd)?;
    let credentials = rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(AppCredentialStore),
    );
    let provider_result = rho_providers::providers::build_sdk_provider_with_source(
        sdk_options.provider,
        &credentials,
    );
    let (missing_auth_error, missing_auth_model_error) = match provider_result {
        Ok(_) => (None, None),
        Err(error) if is_interactive_startup_unavailable_error(&error) => {
            (Some(error.to_string()), Some(error))
        }
        Err(error) => return Err(error.into()),
    };
    interactive::run(interactive::Startup {
        cli: startup.cli,
        catalog: startup.catalog,
        config: startup.config,
        config_path: absolute_config_path(&startup.config_repository)?,
        config_repository: startup.config_repository,
        cwd: startup.cwd,
        first_run: startup.first_run,
        missing_auth_error,
        missing_auth_model_error,
        pending_update_notice,
        diagnostics,
        herdr: startup.herdr,
        agent: startup.bound_agent,
        reasoning_source: startup.bound_reasoning_source,
    })
    .await
}

fn bind_agent_diagnostics(
    config: &crate::config::Config,
    agent: &super::agent_binding::BoundAgent,
) -> RuntimeDiagnostics {
    let diagnostics = RuntimeDiagnostics::new(config);
    diagnostics.update_agent(agent.id().as_str(), &agent.fingerprint().to_string());
    diagnostics
}

fn ensure_cli_credential_store_choice(
    config: &mut crate::config::Config,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use rho_providers::credentials::CredentialStoreBackend;
    use std::io::{self, IsTerminal, Write};

    let Some(request) = crate::credential_store::choice_request(config) else {
        return Ok(());
    };

    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        anyhow::bail!(
            "credential store is unset; set it before non-interactive login with \
`rho credential-store set os|file`, behavior.credential_store in config.toml, \
or RHO_CREDENTIAL_STORE=os|file"
        );
    }

    let backends = request.available_backends();
    if backends.is_empty() {
        anyhow::bail!(
            "no credential store backend is available (os: {}; file: {})",
            request.os.detail,
            request.file.detail
        );
    }

    eprintln!("Choose where Rho stores provider credentials:");
    eprintln!("This is saved to config and used for future logins on this machine.");
    if request.os.available {
        eprintln!("  [1] OS credential store (recommended)");
    } else {
        eprintln!(
            "  [1] OS credential store (unavailable: {})",
            request.os.detail
        );
    }
    if request.file.available {
        eprintln!("  [2] Local file under ~/.rho/credentials (not encrypted at rest)");
    } else {
        eprintln!("  [2] Local file (unavailable: {})", request.file.detail);
    }
    let default_backend = request
        .default_backend()
        .unwrap_or(CredentialStoreBackend::Os);
    let default_hint = match default_backend {
        CredentialStoreBackend::Os => "1",
        CredentialStoreBackend::File => "2",
    };
    eprint!("Choice [1/2 or os/file] (default {default_hint}): ");
    io::stderr().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let backend = match answer.trim() {
        "" => default_backend,
        "1" | "os" | "OS" => CredentialStoreBackend::Os,
        "2" | "file" | "FILE" => CredentialStoreBackend::File,
        other => {
            anyhow::bail!("unrecognized credential store choice '{other}'; expected 1/os or 2/file")
        }
    };
    if !backends.contains(&backend) {
        let detail = request.detail_for(backend);
        anyhow::bail!(
            "{} credential store is unavailable: {detail}",
            backend.as_str()
        );
    }

    let path = crate::credential_store::set_backend(backend, config_path)?;
    config.credential_store = Some(backend);
    eprintln!(
        "credential store set to {} in {}",
        backend.as_str(),
        path.display()
    );
    Ok(())
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

pub(super) fn host_capabilities(
    cli: &Cli,
    config: &crate::config::Config,
    role: AgentRole,
) -> crate::agent::AgentCapabilities {
    use crate::agent::ToolCapability;

    if cli.no_tools {
        return crate::agent::AgentCapabilities::default();
    }
    let mut tools = crate::agent::AgentCapabilities::all_host_tools();
    // web_search is gated after bind against the resolved provider/model.
    #[cfg(windows)]
    tools.remove(&ToolCapability::Bash);
    #[cfg(not(windows))]
    tools.remove(&ToolCapability::Powershell);
    if cli.no_subagents || !config.enable_subagents {
        tools.remove(&ToolCapability::Agent);
        tools.remove(&ToolCapability::Agents);
    }
    if role != AgentRole::InteractiveRoot {
        tools.remove(&ToolCapability::Questionnaire);
    }
    #[cfg(debug_assertions)]
    if std::env::var_os("RHO_TUI_TEST_MODE").as_deref() == Some(std::ffi::OsStr::new("matrix")) {
        tools.insert(ToolCapability::Extension(
            crate::tools::tui_fixture::NAME.into(),
        ));
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
    if cli.command.is_none() && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
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
