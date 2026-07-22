use std::{
    io::{self, IsTerminal},
    sync::Arc,
};

use {
    crate::cli::{Cli, Command, CredentialStoreCommand, OutputFormat},
    crate::credential_store::AppCredentialStore,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::HerdrReporter,
    crate::update,
    rho_providers::model::ModelError,
};

use super::{
    agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    automation, automation_protocol, cli_config,
    config_repository::ConfigRepository,
    interactive, login,
    sdk_config::SdkBootstrapOptions,
};

pub async fn run(cli: Cli) -> anyhow::Result<()> {
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
        automation::emit_startup_failure()?;
        return Err(automation::AutomationExit::new(
            2,
            automation_protocol::TerminalReason::ConfigurationError,
            "configuration failed",
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
    if let Some(Command::CredentialStore { command }) = &cli.command {
        return run_credential_store_command(command, cli.config.clone());
    }
    if let Some(Command::Attach { id }) = &cli.command {
        return crate::tui::run_attachment(id, HerdrReporter::from_env()).await;
    }
    if matches!(cli.command, Some(Command::Update)) {
        return update::run_update(env!("CARGO_PKG_VERSION")).await;
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
        return login::run(provider, *device_auth).await;
    }

    let config_path = cli.config.clone();
    let config_repository = ConfigRepository::new(config_path.clone());
    let mut config = config_repository.load()?;
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
    let catalog = crate::agent::AgentCatalog::discover(&cwd)?;
    let selected_agent = cli.agent.as_deref().unwrap_or("default");
    let definition = Arc::new(catalog.find(selected_agent)?.definition.clone());

    let store = AppCredentialStore;
    let provider_refresh = cli_config::refresh_model_cache(&cli, &config, &store).await?;
    let mut save_config = cli_config::apply_overrides(&mut config, &cli)?;
    cli_config::prepare_model_metadata(&config, &store, &provider_refresh).await;
    save_config |= cli_config::normalize_reasoning_for_cli(
        &mut config,
        if cli.reasoning.is_some() {
            rho_providers::model::ReasoningRequestSource::Explicit
        } else {
            rho_providers::model::ReasoningRequestSource::PersistedOrDefault
        },
    )?;
    if save_config {
        config_repository.save(&config)?;
    }
    let reasoning_before_binding = config.reasoning;
    let role = if automation_prompt.is_some() {
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
    config = bound_agent.config().clone();

    validate_terminal_mode(&cli)?;
    cli_config::prepare_model_metadata(&config, &store, &provider_refresh).await;
    let bound_reasoning_source =
        if cli.reasoning.is_some() && config.reasoning == reasoning_before_binding {
            rho_providers::model::ReasoningRequestSource::Explicit
        } else {
            rho_providers::model::ReasoningRequestSource::PersistedOrDefault
        };
    cli_config::normalize_reasoning_for_cli(&mut config, bound_reasoning_source)?;
    let herdr = HerdrReporter::from_env();
    if let Some(prompt) = automation_prompt {
        let diagnostics = RuntimeDiagnostics::new(&config);
        diagnostics.update_agent(
            bound_agent.id().as_str(),
            &bound_agent.fingerprint().to_string(),
        );
        return automation::run(
            prompt,
            automation::Startup {
                config: &config,
                config_path: absolute_config_path(&config_repository)?,
                cwd,
                no_system_prompt: cli.no_system_prompt,
                no_tools: cli.no_tools,
                no_subagents: cli.no_subagents,
                usage_purpose: "agent",
                parent_session_id: None,
                agent: bound_agent,
                output_file,
                output,
                max_steps,
                timeout,
                diagnostics,
                herdr,
            },
        )
        .await;
    }
    let diagnostics = RuntimeDiagnostics::new(&config);
    diagnostics.update_agent(
        bound_agent.id().as_str(),
        &bound_agent.fingerprint().to_string(),
    );

    let pending_update_notice = config
        .check_for_updates
        .then(|| tokio::spawn(update::update_notice(env!("CARGO_PKG_VERSION"))));

    let sdk_options = SdkBootstrapOptions::from_config(&config, &cwd)?;
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
    let result = interactive::run(interactive::Startup {
        cli: &cli,
        config,
        config_path: absolute_config_path(&config_repository)?,
        config_repository,
        cwd,
        missing_auth_error,
        missing_auth_model_error,
        pending_update_notice,
        diagnostics,
        herdr,
        agent: bound_agent,
        reasoning_source: bound_reasoning_source,
    })
    .await;
    result
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

fn host_capabilities(
    cli: &Cli,
    config: &crate::config::Config,
    role: AgentRole,
) -> crate::agent::AgentCapabilities {
    use crate::agent::ToolCapability;

    if cli.no_tools {
        return crate::agent::AgentCapabilities::default();
    }
    let mut tools = crate::agent::AgentCapabilities::all_host_tools();
    if !crate::tools::web::access_tools(config).is_available() {
        tools.remove(&ToolCapability::WebSearch);
    }
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

fn absolute_config_path(repository: &ConfigRepository) -> anyhow::Result<std::path::PathBuf> {
    let path = repository.configured_path()?;
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
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
        ModelError::MissingApiKey
            | ModelError::MissingCodexAuth
            | ModelError::MissingAnthropicApiKey
            | ModelError::MissingGithubCopilotAuth
            | ModelError::MissingXaiApiKey
            | ModelError::MissingXaiAuth
            | ModelError::Credentials(_)
            | ModelError::UnsupportedProvider(_)
    )
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
