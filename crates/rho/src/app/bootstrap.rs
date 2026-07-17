use std::{
    collections::BTreeSet,
    io::{self, IsTerminal},
    sync::Arc,
};

use crate::{
    cli::{Cli, Command},
    credentials::OsCredentialStore,
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
    model::{models_dev::cached_model_metadata, ModelError},
    update,
};

use super::{
    agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    automation, cli_config,
    config_repository::ConfigRepository,
    interactive, login,
    sdk_config::SdkBootstrapOptions,
};

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    cli_config::validate(&cli)?;
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
        return login::run(provider, *device_auth).await;
    }

    let config_path = cli.config.clone();
    let config_repository = ConfigRepository::new(config_path.clone());
    let mut config = config_repository.load()?;
    let cwd = std::env::current_dir()?;
    let automation_prompt = automation::prompt_for_command(&cli.command)?;
    let output_file = match &cli.command {
        Some(Command::Run { output_file, .. }) => output_file.clone(),
        _ => None,
    };
    let catalog = crate::agent::AgentCatalog::discover(&cwd)?;
    let selected_agent = cli.agent.as_deref().unwrap_or("default");
    let definition = Arc::new(catalog.find(selected_agent)?.definition.clone());

    let store = OsCredentialStore;
    cli_config::refresh_model_cache(&cli, &store).await?;
    if cli_config::apply_overrides(&mut config, &cli)? {
        config_repository.save(&config)?;
    }
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
    if automation_prompt.is_some()
        && config.provider == "anthropic"
        && cached_model_metadata(&config.provider, &config.model).is_none()
    {
        let _ =
            crate::model::models_dev::fetch_model_metadata(&config.provider, &config.model).await;
    }
    cli_config::normalize_reasoning(&mut config);
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
    let credentials = crate::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(OsCredentialStore),
    );
    let provider_result =
        crate::providers::build_sdk_provider_with_source(sdk_options.provider, &credentials);
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
    })
    .await;
    result
}

fn host_capabilities(
    cli: &Cli,
    config: &crate::config::Config,
    role: AgentRole,
) -> BTreeSet<String> {
    if cli.no_tools {
        return BTreeSet::new();
    }
    let mut tools = crate::agent::KNOWN_TOOLS
        .iter()
        .map(|tool| (*tool).to_string())
        .collect::<BTreeSet<_>>();
    if !crate::tools::web::access_tools(config).is_available() {
        tools.remove("web_search");
    }
    #[cfg(windows)]
    tools.remove("bash");
    #[cfg(not(windows))]
    tools.remove("powershell");
    if cli.no_subagents || !config.enable_subagents {
        tools.remove("agent");
        tools.remove("agents");
    }
    if role != AgentRole::InteractiveRoot {
        tools.remove("questionnaire");
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
